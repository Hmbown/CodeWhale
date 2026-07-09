//! OpenAI Codex / ChatGPT OAuth credential loading and token refresh.
//!
//! Reads existing Codex CLI credentials from `~/.codex/auth.json` (or
//! `$CODEX_HOME/auth.json`) and transparently refreshes expired access tokens
//! using the OpenAI auth endpoint.
//!
//! # Security
//!
//! Token values are never logged or printed. All debug representations
//! redact sensitive fields.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

/// OAuth token payload stored in `auth.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AuthTokens {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    account_id: Option<String>,
}

/// Top-level structure of Codex CLI's `auth.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexAuthFile {
    tokens: Option<AuthTokens>,
    last_refresh: Option<String>,
}

/// Resolved OAuth credentials ready for API use.
#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub account_id: Option<String>,
}

/// JWT claims subset for expiry extraction.
#[derive(Debug, Deserialize)]
struct JwtClaims {
    exp: Option<u64>,
}

/// Resolve the path to the Codex auth file.
///
/// Priority:
/// 1. `OPENAI_CODEX_AUTH_FILE` env var
/// 2. `$CODEX_HOME/auth.json`
/// 3. `~/.codex/auth.json`
pub fn auth_file_path() -> PathBuf {
    if let Ok(path) = std::env::var("OPENAI_CODEX_AUTH_FILE") {
        let p = PathBuf::from(&path);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    codex_home.join("auth.json")
}

/// Try to extract `exp` (epoch seconds) from a JWT without verifying
/// the signature. Returns `None` on any parse failure.
fn jwt_expiry_seconds(token: &str) -> Option<u64> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = parts[1];
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: JwtClaims = serde_json::from_slice(&decoded).ok()?;
    claims.exp
}

/// Check whether an access token is expired, with a 60-second safety margin.
fn token_is_expired(access_token: &str) -> bool {
    match jwt_expiry_seconds(access_token) {
        Some(exp) => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_secs();
            // 60-second safety margin
            now + 60 >= exp
        }
        // If we can't parse expiry, assume it might be expired — try refresh.
        None => true,
    }
}

/// Load Codex credentials from the auth file.
///
/// Returns `Ok(None)` if the file doesn't exist or has no usable tokens.
/// Returns `Err` only on parse/IO errors that aren't "file not found".
pub fn load_credentials() -> Result<Option<CodexCredentials>> {
    let path = auth_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path)
        .with_context(|| format!("reading Codex auth file: {}", path.display()))?;
    let auth: CodexAuthFile = serde_json::from_str(&contents)
        .with_context(|| format!("parsing Codex auth file: {}", path.display()))?;
    let tokens = match auth.tokens {
        Some(t) => t,
        None => return Ok(None),
    };
    let access_token = match tokens.access_token {
        Some(t) if !t.trim().is_empty() => t,
        _ => return Ok(None),
    };
    Ok(Some(CodexCredentials {
        access_token,
        refresh_token: tokens.refresh_token,
        account_id: tokens.account_id,
    }))
}

/// Refresh an expired access token using the refresh token.
///
/// Calls the OpenAI token endpoint and returns new credentials.
/// On success, updates the auth file on disk. Synchronous (blocking) so it can
/// run inside the prompt-free, sync config credential-resolution path, matching
/// the Kimi OAuth refresh flow.
fn refresh_access_token(refresh_token: &str) -> Result<CodexCredentials> {
    let client = crate::tls::reqwest_blocking_client_builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("building token refresh client")?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", CODEX_CLIENT_ID),
    ];
    let response = client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .context("sending token refresh request")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        bail!("Token refresh failed (HTTP {status}): {body}");
    }
    let body: serde_json::Value = response.json().context("parsing token refresh response")?;
    let new_access = body["access_token"]
        .as_str()
        .context("missing access_token in refresh response")?
        .to_string();
    let new_refresh = body["refresh_token"].as_str().map(ToOwned::to_owned);
    let new_id = body["id_token"].as_str().map(ToOwned::to_owned);

    // Extract account_id from id_token if available.
    let account_id = new_id.as_deref().and_then(extract_account_id_from_id_token);

    let creds = CodexCredentials {
        access_token: new_access,
        refresh_token: new_refresh.or_else(|| Some(refresh_token.to_string())),
        account_id,
    };

    // Persist refreshed credentials.
    if let Err(e) = save_credentials(&creds, new_id.as_deref()) {
        tracing::warn!("Failed to persist refreshed Codex credentials: {e}");
    }

    Ok(creds)
}

/// Extract `chatgpt_account_id` from the `https://api.openai.com/auth`
/// JWT claim namespace.
fn extract_account_id_from_id_token(id_token: &str) -> Option<String> {
    let parts: Vec<&str> = id_token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value
        .get("https://api.openai.com/auth")?
        .get("chatgpt_account_id")?
        .as_str()
        .map(ToOwned::to_owned)
}

/// Save credentials back to the auth file, preserving file permissions.
fn save_credentials(creds: &CodexCredentials, id_token: Option<&str>) -> Result<()> {
    let path = auth_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating Codex auth dir: {}", parent.display()))?;
    }
    let auth = CodexAuthFile {
        tokens: Some(AuthTokens {
            access_token: Some(creds.access_token.clone()),
            refresh_token: creds.refresh_token.clone(),
            id_token: id_token.map(ToOwned::to_owned),
            account_id: creds.account_id.clone(),
        }),
        last_refresh: Some(chrono_humanize_if_available()),
    };
    let json = serde_json::to_string_pretty(&auth).context("serializing credentials")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut file = opts
            .open(&path)
            .with_context(|| format!("writing Codex auth file: {}", path.display()))?;
        std::io::Write::write_all(&mut file, json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &json)
            .with_context(|| format!("writing Codex auth file: {}", path.display()))?;
    }
    Ok(())
}

fn chrono_humanize_if_available() -> String {
    // Simple ISO-ish timestamp without adding a chrono dependency.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| format!("{} seconds since epoch", d.as_secs()))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Load or refresh Codex credentials.
///
/// 1. Try env overrides first (`OPENAI_CODEX_ACCESS_TOKEN` / `CODEX_ACCESS_TOKEN`).
/// 2. Load from auth file.
/// 3. If access token is expired and refresh token is available, refresh.
///
/// Synchronous so it can be called from the prompt-free config credential
/// resolution path (mirrors the Kimi OAuth flow).
pub fn get_credentials() -> Result<CodexCredentials> {
    // Env override takes priority.
    if let Ok(token) = std::env::var("OPENAI_CODEX_ACCESS_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(CodexCredentials {
            access_token: token,
            refresh_token: None,
            account_id: codex_account_id_env(),
        });
    }
    if let Ok(token) = std::env::var("CODEX_ACCESS_TOKEN")
        && !token.trim().is_empty()
    {
        return Ok(CodexCredentials {
            access_token: token,
            refresh_token: None,
            account_id: codex_account_id_env(),
        });
    }

    let creds = load_credentials()?.with_context(missing_auth_message)?;

    // Check if the access token is still valid.
    if !token_is_expired(&creds.access_token) {
        return Ok(creds);
    }

    // Try refreshing.
    match creds.refresh_token {
        Some(ref rt) if !rt.trim().is_empty() => {
            tracing::info!("Codex access token expired, refreshing...");
            refresh_access_token(rt)
        }
        _ => bail!(
            "Codex access token expired and no refresh token available.\n\
             Run `codex login` to re-authenticate."
        ),
    }
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "OpenAI Codex OAuth credentials not found.\n\
         \n\
         CodeWhale checked OPENAI_CODEX_ACCESS_TOKEN, CODEX_ACCESS_TOKEN, and {}.\n\
         Run `codex login` to authenticate with ChatGPT/Codex OAuth, or set OPENAI_CODEX_ACCESS_TOKEN for this process.",
        auth_file_path().display()
    )
}

/// Best-effort ChatGPT account id for the `chatgpt-account-id` request header.
///
/// Resolves from env overrides first, then the on-disk auth file. Never
/// refreshes and never errors — a missing account id just means the header is
/// omitted.
pub fn codex_account_id() -> Option<String> {
    if let Some(id) = codex_account_id_env() {
        return Some(id);
    }
    load_credentials().ok().flatten().and_then(|c| c.account_id)
}

/// Read a ChatGPT account id from env overrides only.
fn codex_account_id_env() -> Option<String> {
    for var in ["OPENAI_CODEX_ACCOUNT_ID", "CODEX_ACCOUNT_ID"] {
        if let Ok(value) = std::env::var(var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// OpenAI OAuth constants (from Codex CLI reference implementation).
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jwt_expiry_parses_valid_token() {
        // A minimal JWT with {"exp": 9999999999} as payload.
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":9999999999}");
        let token = format!("header.{payload}.signature");
        assert_eq!(jwt_expiry_seconds(&token), Some(9999999999));
    }

    #[test]
    fn jwt_expiry_returns_none_for_malformed() {
        assert_eq!(jwt_expiry_seconds("not.a.jwt"), None);
        assert_eq!(jwt_expiry_seconds(""), None);
        assert_eq!(jwt_expiry_seconds("x"), None);
    }

    #[test]
    fn token_is_expired_detects_future() {
        // Far future — should not be expired.
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":9999999999}");
        let token = format!("header.{payload}.sig");
        assert!(!token_is_expired(&token));
    }

    #[test]
    fn token_is_expired_detects_past() {
        // Way in the past.
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":1000000000}");
        let token = format!("header.{payload}.sig");
        assert!(token_is_expired(&token));
    }

    #[test]
    fn auth_file_path_respects_env() {
        // Just verify it returns a path without panicking.
        let path = auth_file_path();
        assert!(path.to_string_lossy().contains("auth.json"));
    }

    #[test]
    fn missing_auth_message_mentions_oauth_checked_locations() {
        let message = missing_auth_message();

        assert!(message.contains("OpenAI Codex OAuth credentials not found"));
        assert!(message.contains("OPENAI_CODEX_ACCESS_TOKEN"));
        assert!(message.contains("CODEX_ACCESS_TOKEN"));
        assert!(message.contains(&auth_file_path().display().to_string()));
        assert!(message.contains("codex login"));
    }
}
