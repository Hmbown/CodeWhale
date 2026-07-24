//! xAI / Grok OAuth credential loading, refresh, and device-code login.
//!
//! Two paths, matching [#4257](https://github.com/Hmbown/CodeWhale/issues/4257):
//!
//! 1. **Read-only external login** — reuse one exact official Grok CLI token
//!    file only after provider-scoped consent. External tokens are never
//!    refreshed or rewritten.
//! 2. **Native device-code** — request a code from `auth.x.ai`, print the
//!    verification URL + user code, poll the token endpoint, and write tokens
//!    to Codewhale-owned storage.
//!
//! Access tokens are sent as `Authorization: Bearer` on the OpenAI-compatible
//! xAI Chat Completions route (`https://api.x.ai/v1`). Token values are never
//! logged.

use std::collections::BTreeMap;
#[cfg(test)]
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{ApiProvider, Config};

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

/// A successful device-code exchange that has not yet been made active.
///
/// Keeping the bearer material in memory until activation lets the config
/// pointer and a uniquely named owned credential generation commit as one
/// logical operation. A cancelled or failed finalization never leaves a
/// canonical token file that becomes active on a later launch.
#[derive(Debug)]
pub struct PendingXaiDeviceLogin {
    issuer: String,
    client_id: String,
    token: TokenResponse,
}

/// Receipt for the committed Codewhale-owned xAI OAuth generation.
#[derive(Debug)]
pub struct XaiDeviceActivation {
    #[allow(dead_code)]
    pub credentials: XaiOAuthCredentials,
    pub config_path: PathBuf,
    pub auth_path: PathBuf,
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
                return codewhale_config::resolve_external_credential_path(&p).unwrap_or(p);
            }
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let p = PathBuf::from(home.trim());
        if !p.as_os_str().is_empty() {
            let path = p.join("auth.json");
            return codewhale_config::resolve_external_credential_path(&path).unwrap_or(path);
        }
    }
    let path = crate::config::effective_home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("auth.json");
    codewhale_config::resolve_external_credential_path(&path).unwrap_or(path)
}

/// Codewhale-owned xAI token file. Native login and refresh never target the
/// Grok CLI's file.
pub fn codewhale_auth_file_path() -> Result<PathBuf> {
    codewhale_config::legacy_xai_oauth_path()
}

fn configured_owned_auth_file_path(config: &Config) -> Result<Option<PathBuf>> {
    let generation = config
        .provider_config_for(ApiProvider::Xai)
        .and_then(|entry| entry.oauth_credential_generation.as_deref());
    match generation {
        Some(generation) => codewhale_config::xai_oauth_generation_path(generation).map(Some),
        None => Ok(None),
    }
}

#[must_use]
pub fn credentials_present(config: &Config) -> bool {
    credentials_valid(config)
}

/// Prompt-free structural check for xAI OAuth material. Never refreshes,
/// writes, or makes network requests. External storage is not inspected until
/// exact read-only consent has been validated.
#[must_use]
pub fn credentials_valid(config: &Config) -> bool {
    // Codewhale-owned OAuth bytes are inert until the xAI provider explicitly
    // selects OAuth. A failed post-login config finalization can therefore
    // never make a newly written token silently ready on the next launch.
    if !config
        .provider_config_for(ApiProvider::Xai)
        .and_then(|entry| entry.auth_mode.as_deref())
        .is_some_and(auth_mode_uses_xai_oauth)
    {
        return false;
    }
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
    if config
        .provider_config_for(ApiProvider::Xai)
        .and_then(|entry| entry.oauth_credential_generation.as_deref())
        .is_some()
    {
        // A configured generation is authoritative. Invalid, missing, unsafe,
        // or malformed owned storage must not fall through to an external CLI.
        return false;
    }
    if let Ok(path) = codewhale_auth_file_path()
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

    let path = auth_file_path();
    let Ok(grant) = config.external_credential_read_grant(
        ApiProvider::Xai,
        codewhale_config::ExternalCredentialSource::GrokCli,
        &path,
    ) else {
        return false;
    };
    let Ok(mut file) = load_external_auth_file(&grant) else {
        return false;
    };
    select_entry(&mut file).is_some_and(|(_, entry)| entry_access_token_is_fresh(&entry))
}

/// Load xAI OAuth credentials. Codewhale-owned credentials may refresh and
/// rewrite Codewhale-owned storage. External credentials are read-only.
pub fn get_access_token(config: &Config) -> Result<String> {
    Ok(get_credentials(config)?.access_token)
}

pub fn get_credentials(config: &Config) -> Result<XaiOAuthCredentials> {
    anyhow::ensure!(
        config.api_provider() == ApiProvider::Xai
            && config
                .provider_config_for(ApiProvider::Xai)
                .and_then(|entry| entry.auth_mode.as_deref())
                .is_some_and(auth_mode_uses_xai_oauth),
        "Codewhale-owned xAI OAuth credentials are inactive until the xAI route explicitly selects OAuth"
    );
    if let Some(owned_path) = configured_owned_auth_file_path(config)? {
        return get_owned_credentials(&owned_path);
    }
    let owned_path = codewhale_auth_file_path()?;
    if load_owned_auth_file(&owned_path)?.is_some() {
        return get_owned_credentials(&owned_path);
    }

    let external_path = auth_file_path();
    let grant = config.external_credential_read_grant(
        ApiProvider::Xai,
        codewhale_config::ExternalCredentialSource::GrokCli,
        &external_path,
    )?;
    let mut file = load_external_auth_file(&grant)?;
    let (scope, entry) = select_entry(&mut file).ok_or_else(|| {
        anyhow::anyhow!(
            "xAI OAuth credentials at {} have no usable entry. Run `grok login` again or use `codewhale auth xai-device` for Codewhale-owned storage.",
            codewhale_config::quote_os_path(grant.path())
        )
    })?;
    if !entry_access_token_is_fresh(&entry) {
        bail!(
            "xAI OAuth access token in {} is expired. Read-only consent never refreshes or rewrites another CLI's credentials. Run `grok login` again or use `codewhale auth xai-device`.",
            codewhale_config::quote_os_path(grant.path())
        );
    }
    let token = entry
        .key
        .clone()
        .filter(|token| !token.trim().is_empty())
        .context("xAI OAuth access token is empty")?;
    Ok(credentials_from_entry(scope, &entry, token))
}

fn get_owned_credentials(path: &Path) -> Result<XaiOAuthCredentials> {
    let directory = codewhale_config::xai_oauth_credentials_dir()?;
    anyhow::ensure!(
        path.parent() == Some(directory.as_path()),
        "Codewhale-owned xAI OAuth path escaped the credentials directory"
    );
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Codewhale-owned xAI OAuth path must have a UTF-8 basename")?;
    anyhow::ensure!(
        name == codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME
            || codewhale_config::is_valid_xai_oauth_generation(name),
        "Codewhale-owned xAI OAuth path has an invalid basename"
    );
    codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
        get_owned_credentials_locked(store, name, refresh_access_token)
    })
}

fn get_owned_credentials_locked<F>(
    store: &codewhale_config::XaiOAuthCredentialStore,
    name: &str,
    refresh_access: F,
) -> Result<XaiOAuthCredentials>
where
    F: FnOnce(&str, &str, &str) -> Result<TokenResponse>,
{
    let path = store.path_for(name)?;
    let mut file = load_owned_auth_file_from_store(store, name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Codewhale-owned xAI OAuth credentials were not found at {}. Run `codewhale auth xai-device` again.",
            codewhale_config::quote_os_path(&path)
        )
    })?;
    let (scope, mut entry) = select_entry(&mut file).ok_or_else(|| {
        anyhow::anyhow!(
            "Codewhale-owned xAI OAuth credentials at {} have no usable entry. Run `codewhale auth xai-device` again.",
            codewhale_config::quote_os_path(&path)
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

    let refreshed = refresh_access(&issuer, &client_id, refresh)?;
    apply_token_response(&mut entry, &issuer, &client_id, &refreshed)?;
    file.insert(scope.clone(), entry.clone());
    write_auth_file_to_store(store, name, &file, true)?;

    let token = entry
        .key
        .clone()
        .filter(|t| !t.trim().is_empty())
        .context("xAI OAuth refresh returned an empty access token")?;
    Ok(credentials_from_entry(scope, &entry, token))
}

/// Interactive device-code login. Prints verification URL + user code to
/// `stderr` and polls until approved. The returned bearer material remains
/// pending in memory until [`activate_device_login`] commits an owned
/// generation and its config pointer.
///
/// Public residual entry point for CLI/TUI wiring (`codewhale auth` /
/// slash command). Call from a headless or TUI surface that can print the
/// verification URL.
pub async fn device_code_login() -> Result<PendingXaiDeviceLogin> {
    let issuer = std::env::var("GROK_OIDC_ISSUER")
        .or_else(|_| std::env::var("XAI_OIDC_ISSUER"))
        .unwrap_or_else(|_| XAI_OIDC_ISSUER.to_string());
    let client_id = std::env::var("GROK_OIDC_CLIENT_ID")
        .or_else(|_| std::env::var("XAI_OIDC_CLIENT_ID"))
        .unwrap_or_else(|_| GROK_OIDC_CLIENT_ID.to_string());
    let scopes = std::env::var("GROK_OIDC_SCOPES")
        .or_else(|_| std::env::var("XAI_OIDC_SCOPES"))
        .unwrap_or_else(|_| DEFAULT_SCOPES.to_string());
    let open_browser = std::env::var_os("CODEWHALE_XAI_OAUTH_NO_BROWSER").is_none();

    device_code_login_on_blocking_thread(issuer, client_id, scopes, open_browser).await
}

async fn device_code_login_on_blocking_thread(
    issuer: String,
    client_id: String,
    scopes: String,
    open_browser: bool,
) -> Result<PendingXaiDeviceLogin> {
    tokio::task::spawn_blocking(move || {
        device_code_login_with(&issuer, &client_id, &scopes, open_browser)
    })
    .await
    .context("xAI device-code login worker failed")?
}

fn device_code_login_with(
    issuer: &str,
    client_id: &str,
    scopes: &str,
    open_browser: bool,
) -> Result<PendingXaiDeviceLogin> {
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
                return Ok(PendingXaiDeviceLogin {
                    issuer: issuer.to_string(),
                    client_id: client_id.to_string(),
                    token,
                });
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

/// Commit a pending device login as a uniquely named owned generation and
/// atomically point `[providers.xai]` at it under the shared config lock.
///
/// The credential file is staged while the config lock is held. If config
/// persistence fails, the unreferenced stage is removed. Only after the new
/// pointer commits is the previously selected generation removed best-effort.
pub fn activate_device_login(
    pending: PendingXaiDeviceLogin,
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
) -> Result<XaiDeviceActivation> {
    codewhale_config::with_xai_oauth_lifecycle_lock(move |store| {
        activate_device_login_locked(pending, config_path, live_config, store)
    })
}

fn activate_device_login_locked(
    pending: PendingXaiDeviceLogin,
    config_path: Option<&Path>,
    live_config: Option<&mut Config>,
    store: &codewhale_config::XaiOAuthCredentialStore,
) -> Result<XaiDeviceActivation> {
    let config_path = crate::config_persistence::config_toml_path(config_path)?;
    let generation = format!(
        "{}{}{}",
        codewhale_config::XAI_OAUTH_GENERATION_PREFIX,
        uuid::Uuid::new_v4().simple(),
        codewhale_config::XAI_OAUTH_GENERATION_SUFFIX
    );
    codewhale_config::validate_xai_oauth_generation(&generation)?;
    let auth_path = store.path_for(&generation)?;
    let key_inside =
        crate::config::provider_config_key(ApiProvider::Xai).context("xAI auth mode key")?;
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
                        "refusing xAI login because the existing credential generation pointer is not a string",
                    )
                    .map(ToOwned::to_owned)
            })
            .transpose()?;
        if let Some(previous) = previous_generation.as_deref() {
            codewhale_config::validate_xai_oauth_generation(previous).with_context(|| {
                "refusing xAI login because the existing credential generation pointer is invalid"
            })?;
        }

        let previous_owned_name = match previous_generation.as_deref() {
            Some(previous) => Some(previous.to_string()),
            None if store
                .read_to_string(codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME)?
                .is_some() =>
            {
                Some(codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME.to_string())
            }
            None => None,
        };
        let mut file = match previous_owned_name.as_deref() {
            Some(name) => load_owned_auth_file_from_store(store, name)?.ok_or_else(|| {
                let path = store.directory().join(name);
                anyhow::anyhow!(
                    "the active Codewhale-owned xAI OAuth generation is missing at {}",
                    codewhale_config::quote_os_path(&path)
                )
            })?,
            None => BTreeMap::new(),
        };
        let scope = format!("{}::{}", pending.issuer, pending.client_id);
        let mut entry = file.remove(&scope).unwrap_or(GrokAuthEntry {
            key: None,
            refresh_token: None,
            expires_at: None,
            oidc_issuer: Some(pending.issuer.clone()),
            oidc_client_id: Some(pending.client_id.clone()),
            auth_mode: Some("oidc".to_string()),
            extra: BTreeMap::new(),
        });
        apply_token_response(
            &mut entry,
            &pending.issuer,
            &pending.client_id,
            &pending.token,
        )?;
        let access = entry
            .key
            .clone()
            .filter(|token| !token.trim().is_empty())
            .context("xAI device-code login returned an empty access token")?;
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
        Ok((
            previous_owned_name,
            credentials_from_entry(scope, &entry, access),
        ))
    });

    let (previous_owned_name, credentials) = match activation {
        Ok(activation) => activation,
        Err(error) => {
            if stage_written && let Err(cleanup_error) = store.remove(&generation) {
                return Err(error).context(format!(
                    "xAI login was not activated; also failed to remove unreferenced staged credentials at {}: {cleanup_error}",
                    codewhale_config::quote_os_path(&auth_path)
                ));
            }
            return Err(error)
                .context("xAI login was not activated; provider configuration is unchanged");
        }
    };

    if let Some(config) = live_config {
        config.mark_codewhale_owned_xai_oauth(generation.clone());
    }
    if let Some(previous) = previous_owned_name
        && previous != generation
        && let Err(error) = store.remove(&previous)
    {
        tracing::warn!(
            target: "codewhale::xai_oauth",
            error = %error,
            "new xAI OAuth generation committed but superseded generation cleanup failed"
        );
    }
    eprintln!(
        "Signed in. Codewhale-owned credentials activated at {}.",
        codewhale_config::quote_os_path(&auth_path)
    );
    Ok(XaiDeviceActivation {
        credentials,
        config_path,
        auth_path,
    })
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "xAI OAuth credentials not found.\n\
         Options:\n\
         1. Run `codewhale auth xai-device` for Codewhale-owned OAuth storage\n\
         2. To read an existing Grok CLI login without changing it, run \
         `codewhale auth external-consent --provider xai --mode read-only --path {}`\n\
         3. Or use API-key auth: export XAI_API_KEY=... / \
         codewhale auth set --provider xai",
        codewhale_config::quote_os_path(&auth_file_path())
    )
}

// ── internals ──────────────────────────────────────────────────────────────

type AuthFile = BTreeMap<String, GrokAuthEntry>;

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

fn load_external_auth_file(
    grant: &codewhale_config::ExternalCredentialReadGrant,
) -> Result<AuthFile> {
    let Some(raw) = crate::external_credentials::read_to_string(grant)? else {
        bail!(
            "external xAI/Grok credential file not found at {}",
            codewhale_config::quote_os_path(grant.path())
        );
    };
    parse_auth_file(&raw, grant.path())
}

fn parse_auth_file(raw: &str, path: &Path) -> Result<AuthFile> {
    let value: Value = serde_json::from_str(raw).map_err(|_| {
        anyhow::anyhow!(
            "xAI/Grok credential file {} is not valid credential JSON",
            codewhale_config::quote_os_path(path)
        )
    })?;
    let obj = value.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "xAI/Grok credential file {} must be a JSON object of entries",
            codewhale_config::quote_os_path(path)
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        match serde_json::from_value::<GrokAuthEntry>(v.clone()) {
            Ok(entry) => {
                out.insert(k.clone(), entry);
            }
            Err(_) => {
                tracing::warn!(
                    target: "codewhale::xai_oauth",
                    "skipping unreadable xAI auth entry"
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
        serde_json::to_vec_pretty(file).context("serializing xAI OAuth credentials")?;
    store
        .write(name, &serialized, allow_replace)
        .with_context(|| {
            format!(
                "writing xAI OAuth credentials to {}",
                codewhale_config::quote_os_path(&store.directory().join(name))
            )
        })?;
    #[cfg(test)]
    crate::external_credentials::record_owned_credential_write();
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
    #[cfg(test)]
    crate::external_credentials::record_oauth_network();
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
    if parsed.origin() != issuer.origin() {
        bail!("xAI OIDC discovery returned {field} on a different origin than the issuer");
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
    #[cfg(test)]
    crate::external_credentials::record_oauth_refresh();
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
    #[cfg(test)]
    crate::external_credentials::record_oauth_network();
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
    #[cfg(test)]
    crate::external_credentials::record_oauth_network();
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
    #[cfg(test)]
    crate::external_credentials::record_oauth_network();
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

#[cfg(all(unix, test))]
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
        let root = dir.path().canonicalize().expect("canonical temp root");
        let path = root.join("auth.json");
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
        let _home_guard = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &root);
        let _path_guard = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &path);
        let config = Config {
            provider: Some(ApiProvider::Xai.as_str().to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::Xai,
                            codewhale_config::ExternalCredentialSource::GrokCli,
                            path.clone(),
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        crate::external_credentials::reset_side_effect_trap();
        let result = get_credentials(&config);
        let creds = result.expect("load");
        assert_eq!(creds.access_token, "test-access-token");
        assert_eq!(creds.client_id, GROK_OIDC_CLIENT_ID);
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );
    }

    #[test]
    fn disabled_external_grok_credentials_cause_zero_external_io() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().expect("canonical temp root");
        let path = root.join("external-grok-auth.json");
        let raw = serde_json::json!({
            format!("{XAI_OIDC_ISSUER}::{GROK_OIDC_CLIENT_ID}"): {
                "key": "must-never-be-read",
                "refresh_token": "must-never-be-used",
                "expires_at": rfc3339_from_now(3600),
                "future_field": {"preserve": true}
            }
        })
        .to_string();
        fs::write(&path, &raw).unwrap();
        let owned_home = root.join("codewhale-owned");
        let _home_guard = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
        let _path_guard = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &path);
        let config = Config {
            provider: Some(ApiProvider::Xai.as_str().to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        assert!(!credentials_valid(&config));
        let error = get_credentials(&config).expect_err("external access is disabled");
        assert!(error.to_string().contains("are disabled"));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (0, 0)
        );
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 0, 0, 0),
            "disabled external authority must reach no credential or OAuth sink"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), raw);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn expired_read_only_external_credentials_never_refresh_rewrite_or_network() {
        let _guard = crate::test_support::lock_test_env();
        let server = MockServer::start().await;
        let dir = TempDir::new().unwrap();
        let root = dir.path().canonicalize().expect("canonical temp root");
        let path = root.join("external-grok-auth.json");
        let scope = format!("{}::{GROK_OIDC_CLIENT_ID}", server.uri());
        let raw = serde_json::json!({
            scope: {
                "key": "expired-external-access",
                "refresh_token": "must-never-be-submitted",
                "expires_at": rfc3339_from_unix(now_unix_secs().unwrap_or(0) - 3600),
                "oidc_issuer": server.uri(),
                "oidc_client_id": GROK_OIDC_CLIENT_ID,
                "future_field": {"preserve": true}
            }
        })
        .to_string();
        fs::write(&path, &raw).unwrap();
        let owned_home = root.join("codewhale-owned");
        let _home_guard = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &owned_home);
        let _path_guard = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &path);
        let config = Config {
            provider: Some(ApiProvider::Xai.as_str().to_string()),
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("oauth".to_string()),
                    external_credentials: Some(
                        codewhale_config::ExternalCredentialConsentToml::read_only(
                            codewhale_config::ProviderKind::Xai,
                            codewhale_config::ExternalCredentialSource::GrokCli,
                            path.clone(),
                        ),
                    ),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        let error = tokio::task::block_in_place(|| get_credentials(&config))
            .expect_err("read-only external credentials must fail instead of refreshing");
        assert!(
            error
                .to_string()
                .contains("Read-only consent never refreshes")
        );
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (1, 1, 0, 0, 0),
            "read-only external expiry must not reach write, refresh, or network sinks"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), raw);
        assert!(!owned_home.join("credentials/xai-auth.json").exists());
        assert!(
            server
                .received_requests()
                .await
                .expect("recorded requests")
                .is_empty(),
            "external refresh tokens must never be sent over the network"
        );
    }

    #[test]
    fn native_login_storage_is_codewhale_owned() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let grok_path = dir.path().join("external-grok-auth.json");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", dir.path());
        let _grok = crate::test_support::EnvVarGuard::set("GROK_AUTH_PATH", &grok_path);

        let owned = codewhale_auth_file_path().expect("Codewhale-owned auth path");
        assert_eq!(owned, dir.path().join("credentials/xai-auth.json"));
        assert_ne!(owned, auth_file_path());
    }

    fn pending_login(access: &str, refresh: &str) -> PendingXaiDeviceLogin {
        PendingXaiDeviceLogin {
            issuer: XAI_OIDC_ISSUER.to_string(),
            client_id: GROK_OIDC_CLIENT_ID.to_string(),
            token: TokenResponse {
                access_token: Some(access.to_string()),
                refresh_token: Some(refresh.to_string()),
                expires_in: Some(3600),
                error: None,
            },
        }
    }

    fn seed_expired_owned_generation() -> String {
        let generation = "xai-auth-0123456789abcdef0123456789abcdef.json".to_string();
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            let scope = format!("{}::{}", XAI_OIDC_ISSUER, GROK_OIDC_CLIENT_ID);
            let mut file = AuthFile::new();
            file.insert(
                scope,
                GrokAuthEntry {
                    key: Some("expired-access".to_string()),
                    refresh_token: Some("initial-refresh".to_string()),
                    expires_at: Some("1970-01-01T00:00:00.000Z".to_string()),
                    oidc_issuer: Some(XAI_OIDC_ISSUER.to_string()),
                    oidc_client_id: Some(GROK_OIDC_CLIENT_ID.to_string()),
                    auth_mode: Some("oidc".to_string()),
                    extra: BTreeMap::new(),
                },
            );
            write_auth_file_to_store(store, &generation, &file, false)
        })
        .expect("seed expired owned generation");
        generation
    }

    fn seed_legacy_owned_credentials() -> PathBuf {
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            let scope = format!("{}::{}", XAI_OIDC_ISSUER, GROK_OIDC_CLIENT_ID);
            let mut legacy = AuthFile::new();
            legacy.insert(
                scope,
                GrokAuthEntry {
                    key: Some("legacy-access".to_string()),
                    refresh_token: Some("legacy-refresh".to_string()),
                    expires_at: Some(rfc3339_from_now(3600)),
                    oidc_issuer: Some(XAI_OIDC_ISSUER.to_string()),
                    oidc_client_id: Some(GROK_OIDC_CLIENT_ID.to_string()),
                    auth_mode: Some("oidc".to_string()),
                    extra: BTreeMap::new(),
                },
            );
            write_auth_file_to_store(
                store,
                codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME,
                &legacy,
                false,
            )?;
            store.path_for(codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME)
        })
        .expect("seed legacy credentials")
    }

    #[test]
    fn concurrent_refreshes_share_one_rotated_epoch() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let generation = seed_expired_owned_generation();
        let refreshes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let first_generation = generation.clone();
        let first_refreshes = refreshes.clone();
        let first = std::thread::spawn(move || {
            codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
                get_owned_credentials_locked(store, &first_generation, |_, _, refresh| {
                    assert_eq!(refresh, "initial-refresh");
                    first_refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(TokenResponse {
                        access_token: Some("rotated-access".to_string()),
                        refresh_token: Some("rotated-refresh".to_string()),
                        expires_in: Some(3600),
                        error: None,
                    })
                })
            })
        });
        entered_rx.recv().expect("first refresh reached barrier");

        let second_generation = generation.clone();
        let second_refreshes = refreshes.clone();
        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        let second = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
                get_owned_credentials_locked(store, &second_generation, |_, _, _| {
                    second_refreshes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    bail!("second refresh must observe the first thread's committed token")
                })
            })
        });
        attempt_rx.recv().expect("second refresh attempted lock");
        release_tx.send(()).expect("release first refresh");

        let first = first.join().unwrap().expect("first refresh");
        let second = second.join().unwrap().expect("second refresh");
        assert_eq!(first.access_token, "rotated-access");
        assert_eq!(second.access_token, "rotated-access");
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
            let mut file = load_owned_auth_file_from_store(store, &generation)?
                .context("generation must remain active")?;
            let (_, entry) = select_entry(&mut file).context("stored entry")?;
            assert_eq!(entry.refresh_token.as_deref(), Some("rotated-refresh"));
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn logout_waits_for_refresh_then_revokes_the_committed_epoch() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        fs::create_dir_all(&home).unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let generation = seed_expired_owned_generation();
        fs::write(
            home.join("config.toml"),
            format!(
                "[providers.xai]\nauth_mode = \"oauth\"\noauth_credential_generation = \"{generation}\"\n"
            ),
        )
        .unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();

        let refresh_generation = generation.clone();
        let refresh = std::thread::spawn(move || {
            codewhale_config::with_xai_oauth_lifecycle_lock(|store| {
                get_owned_credentials_locked(store, &refresh_generation, |_, _, _| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(TokenResponse {
                        access_token: Some("last-refresh-access".to_string()),
                        refresh_token: Some("last-refresh-rotation".to_string()),
                        expires_in: Some(3600),
                        error: None,
                    })
                })
            })
        });
        entered_rx.recv().expect("refresh reached barrier");

        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        let config_path = home.join("config.toml");
        let logout = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            codewhale_config::with_xai_oauth_revocation_transaction(|| {
                codewhale_config::mutate_config_document(&config_path, |document| {
                    codewhale_config::unset_config_document_value(
                        document,
                        &["providers", "xai", "oauth_credential_generation"],
                    )?;
                    codewhale_config::unset_config_document_value(
                        document,
                        &["providers", "xai", "auth_mode"],
                    )?;
                    Ok(())
                })
            })
        });
        attempt_rx.recv().expect("logout attempted lifecycle lock");
        release_tx.send(()).expect("release refresh");

        assert_eq!(
            refresh.join().unwrap().expect("refresh").access_token,
            "last-refresh-access"
        );
        logout.join().unwrap().expect("logout");
        let auth_path = home.join("credentials").join(&generation);
        assert!(
            !auth_path.exists(),
            "logout must retire the generation written by the preceding refresh"
        );
        let config = fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(!config.contains("oauth_credential_generation"));
        assert!(!config.contains("auth_mode"));
    }

    #[test]
    fn activation_commits_unique_generation_pointer_and_revokes_external_consent() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let config_path = dir.path().join("config.toml");
        let external_path = dir.path().join("grok-external.json");
        fs::write(&external_path, "external owner bytes").unwrap();
        fs::write(
            &config_path,
            format!(
                r#"# operator note
[providers.xai]
model = "grok-code-fast-1" # model note
future_setting = "preserve"

[providers.xai.external_credentials]
access = "read_only"
provider = "xai"
source = "grok_cli"
path = {}
consent_version = 1
"#,
                toml::Value::String(external_path.display().to_string())
            ),
        )
        .unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let consent = codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
            external_path.clone(),
        );
        let mut live = Config {
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    model: Some("grok-code-fast-1".to_string()),
                    external_credentials: Some(consent),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::external_credentials::reset_side_effect_trap();
        let activation = activate_device_login(
            pending_login("activation-access", "activation-refresh"),
            Some(&config_path),
            Some(&mut live),
        )
        .expect("activate login");

        assert_eq!(activation.config_path, config_path);
        let generation = activation
            .auth_path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("generation basename");
        assert!(codewhale_config::is_valid_xai_oauth_generation(generation));
        let persisted = fs::read_to_string(&config_path).unwrap();
        assert!(persisted.contains("# operator note"));
        assert!(persisted.contains("model = \"grok-code-fast-1\" # model note"));
        assert!(persisted.contains("future_setting = \"preserve\""));
        assert!(persisted.contains("auth_mode = \"oauth\""));
        assert!(persisted.contains(&format!("oauth_credential_generation = \"{generation}\"")));
        assert!(!persisted.contains("external_credentials"));
        assert_eq!(
            fs::read_to_string(&external_path).unwrap(),
            "external owner bytes"
        );
        let owned = fs::read_to_string(&activation.auth_path).unwrap();
        assert!(owned.contains("activation-access"));
        assert!(owned.contains("activation-refresh"));
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(&activation.auth_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let live_xai = live.provider_config_for(ApiProvider::Xai).unwrap();
        assert_eq!(live_xai.auth_mode.as_deref(), Some("oauth"));
        assert_eq!(
            live_xai.oauth_credential_generation.as_deref(),
            Some(generation)
        );
        assert!(live_xai.external_credentials.is_none());
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (0, 0, 1, 0, 0),
            "activation must reach exactly the owned write sink"
        );
    }

    #[test]
    fn activation_retires_legacy_owned_file_only_after_config_commit() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[providers.xai]\nmodel = \"grok-4.5\"\n").unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let legacy_path = seed_legacy_owned_credentials();
        assert!(legacy_path.exists());

        let activation = activate_device_login(
            pending_login("new-access", "new-refresh"),
            Some(&config_path),
            None,
        )
        .expect("activate replacement generation");

        assert!(activation.auth_path.exists());
        assert!(
            !legacy_path.exists(),
            "legacy duplicate must be removed after the generation pointer commits"
        );
        let persisted = fs::read_to_string(config_path).unwrap();
        assert!(persisted.contains(activation.auth_path.file_name().unwrap().to_str().unwrap()));
    }

    #[test]
    fn activation_rotation_cleans_only_the_superseded_generation_after_commit() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let config_path = dir.path().join("config.toml");
        fs::write(&config_path, "[providers.xai]\nmodel = \"grok-4.5\"\n").unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let mut live = Config::default();

        let first = activate_device_login(
            pending_login("first-access", "first-refresh"),
            Some(&config_path),
            Some(&mut live),
        )
        .expect("first activation");
        assert!(first.auth_path.exists());
        let first_name = first
            .auth_path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let second = activate_device_login(
            pending_login("second-access", "second-refresh"),
            Some(&config_path),
            Some(&mut live),
        )
        .expect("second activation");
        assert_ne!(first.auth_path, second.auth_path);
        assert!(second.auth_path.exists());
        assert!(
            !first.auth_path.exists(),
            "superseded generation must be removed only after the new pointer commits"
        );
        let persisted = fs::read_to_string(&config_path).unwrap();
        assert!(!persisted.contains(&first_name));
        assert!(persisted.contains(second.auth_path.file_name().unwrap().to_str().unwrap()));
        assert!(
            fs::read_to_string(second.auth_path)
                .unwrap()
                .contains("second-access")
        );
    }

    #[test]
    fn activation_rejects_a_non_string_generation_pointer_without_staging_credentials() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let config_path = dir.path().join("config.toml");
        let original = "[providers.xai]\noauth_credential_generation = { path = \"attacker\" }\n";
        fs::write(&config_path, original).unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);

        let error = activate_device_login(
            pending_login("must-not-stage", "must-not-persist"),
            Some(&config_path),
            None,
        )
        .expect_err("non-string generation pointers must fail closed");
        assert!(error.to_string().contains("not activated"), "{error:#}");
        assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
        let credentials = home.join("credentials");
        assert!(credentials.exists(), "lifecycle lock directory is durable");
        assert!(fs::read_dir(credentials).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            name != codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME
                && !codewhale_config::is_valid_xai_oauth_generation(&name)
        }));
    }

    #[cfg(unix)]
    #[test]
    fn activation_failure_cleans_unreferenced_stage_and_keeps_live_config_inert() {
        let _guard = crate::test_support::lock_test_env();
        let dir = TempDir::new().unwrap();
        let home = dir
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("owned-home");
        let config_dir = dir.path().join("config-parent");
        fs::create_dir(&config_dir).unwrap();
        let config_path = config_dir.join("config.toml");
        fs::write(&config_path, "[providers.xai]\nauth_mode = \"api_key\"\n").unwrap();
        fs::write(config_dir.join("config.toml.lock"), "").unwrap();
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o500)).unwrap();
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &home);
        let legacy_path = seed_legacy_owned_credentials();
        let legacy_before = fs::read(&legacy_path).unwrap();
        let mut live = Config {
            providers: Some(crate::config::ProvidersConfig {
                xai: crate::config::ProviderConfig {
                    auth_mode: Some("api_key".to_string()),
                    api_key: Some("still-selected".to_string()),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = activate_device_login(
            pending_login("must-be-cleaned", "must-not-persist"),
            Some(&config_path),
            Some(&mut live),
        );
        fs::set_permissions(&config_dir, fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.expect_err("read-only config directory must fail activation");
        assert!(error.to_string().contains("not activated"), "{error:#}");
        let live_xai = live.provider_config_for(ApiProvider::Xai).unwrap();
        assert_eq!(live_xai.auth_mode.as_deref(), Some("api_key"));
        assert!(live_xai.oauth_credential_generation.is_none());
        assert_eq!(
            fs::read(&legacy_path).unwrap(),
            legacy_before,
            "legacy owned credentials must remain byte-identical until activation commits"
        );
        let credentials = home.join("credentials");
        if credentials.exists() {
            assert!(
                fs::read_dir(credentials).unwrap().all(|entry| {
                    let name = entry.unwrap().file_name();
                    let name = name.to_string_lossy();
                    name == codewhale_config::LEGACY_XAI_OAUTH_FILE_NAME
                        || !codewhale_config::is_valid_xai_oauth_generation(&name)
                }),
                "failed activation must remove every unreferenced generation but retain legacy"
            );
        }
        assert!(
            !fs::read_to_string(config_path)
                .unwrap()
                .contains("must-be-cleaned")
        );
    }

    #[test]
    fn missing_file_message_mentions_oauth_paths() {
        let _guard = crate::test_support::lock_test_env();
        let msg = missing_auth_message();
        assert!(msg.contains("xAI OAuth credentials not found"), "{msg}");
        assert!(msg.contains("external-consent"), "{msg}");
        assert!(msg.contains("Codewhale-owned OAuth storage"), "{msg}");
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
        let _ = device_code_login;
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
    async fn device_login_discovery_request_is_safe_inside_tokio_runtime() {
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
            .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
                "error": "invalid_scope",
                "error_description": "mock refusal before browser or polling"
            })))
            .expect(1)
            .mount(&server)
            .await;

        let error = device_code_login_on_blocking_thread(
            server.uri(),
            "test-public-client".to_string(),
            "openid".to_string(),
            false,
        )
        .await
        .expect_err("mock device request must fail without a runtime-drop panic");
        let message = format!("{error:#}");

        assert!(message.contains("invalid_scope"), "{message}");
        assert!(message.contains("HTTP 400"), "{message}");
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
    fn https_discovery_accepts_same_origin_with_explicit_default_port() {
        let endpoint = "https://auth.x.ai:443/oauth2/token";
        let validated = validate_discovered_oauth_endpoint(
            Some(endpoint.to_string()),
            "token_endpoint",
            XAI_OIDC_ISSUER,
        )
        .expect("URL origins normalize the explicit default HTTPS port");

        assert_eq!(validated, endpoint);
    }

    #[test]
    fn https_discovery_rejects_cross_origin_endpoint() {
        let error = validate_discovered_oauth_endpoint(
            Some("https://oauth.attacker.example/oauth2/token".to_string()),
            "token_endpoint",
            XAI_OIDC_ISSUER,
        )
        .expect_err("discovered OAuth endpoints must stay on the issuer origin");

        assert!(error.to_string().contains("different origin"), "{error}");
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

        let result = tokio::task::block_in_place(|| {
            device_code_login_with(&server.uri(), GROK_OIDC_CLIENT_ID, DEFAULT_SCOPES, false)
        });

        let pending = result.expect("device login");
        assert_eq!(
            pending.token.access_token.as_deref(),
            Some("test-xai-access")
        );
        assert_eq!(
            pending.token.refresh_token.as_deref(),
            Some("test-xai-refresh")
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

        let result = tokio::task::block_in_place(|| {
            device_code_login_with(&server.uri(), GROK_OIDC_CLIENT_ID, DEFAULT_SCOPES, false)
        });

        let pending = result.expect("device login after pending and slow_down");
        assert_eq!(
            pending.token.access_token.as_deref(),
            Some("test-xai-access")
        );
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

        let result = tokio::task::block_in_place(|| {
            device_code_login_with(&server.uri(), GROK_OIDC_CLIENT_ID, DEFAULT_SCOPES, false)
        });

        let error = result.expect_err("user denial must stop polling");
        let message = format!("{error:#}");
        assert!(message.contains("access_denied"), "{message}");
        assert!(message.contains("HTTP 400"), "{message}");
        assert!(!message.contains("authorization_pending"), "{message}");
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
