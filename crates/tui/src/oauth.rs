//! OpenAI Codex / ChatGPT OAuth credential loading.
//!
//! External Codex CLI credentials are read only after an exact, provider-scoped
//! consent grant. Codewhale never refreshes or rewrites that external file.
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
use codewhale_config::ExternalCredentialReadGrant;
use serde::Deserialize;

/// OAuth token payload stored in `auth.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct AuthTokens {
    access_token: Option<String>,
    account_id: Option<String>,
}

/// Top-level structure of Codex CLI's `auth.json`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
struct CodexAuthFile {
    tokens: Option<AuthTokens>,
}

/// Resolved OAuth credentials ready for API use.
#[derive(Debug, Clone)]
pub struct CodexCredentials {
    pub access_token: String,
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
            return codewhale_config::resolve_external_credential_path(&p).unwrap_or(p);
        }
    }
    let codex_home = std::env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            crate::config::effective_home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        });
    let path = codex_home.join("auth.json");
    codewhale_config::resolve_external_credential_path(&path).unwrap_or(path)
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
        // If we can't prove freshness, fail closed. External credentials are
        // never refreshed by Codewhale.
        None => true,
    }
}

/// Load Codex credentials from the auth file.
///
/// Returns `Ok(None)` if the file doesn't exist or has no usable tokens.
/// Returns `Err` only on parse/IO errors that aren't "file not found".
fn load_credentials(grant: &ExternalCredentialReadGrant) -> Result<Option<CodexCredentials>> {
    let Some(contents) = crate::external_credentials::read_to_string(grant)? else {
        return Ok(None);
    };
    let auth: CodexAuthFile = serde_json::from_str(&contents).map_err(|_| {
        anyhow::anyhow!(
            "Codex credential file {} is not valid credential JSON",
            codewhale_config::quote_os_path(grant.path())
        )
    })?;
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
        account_id: tokens.account_id,
    }))
}

/// Prompt-free, non-refreshing readiness check for picker/onboarding surfaces.
/// It reads process-level token variables only; no file or network access occurs.
#[must_use]
pub fn credentials_from_env() -> Option<CodexCredentials> {
    ["OPENAI_CODEX_ACCESS_TOKEN", "CODEX_ACCESS_TOKEN"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|token| !token.trim().is_empty())
        })
        .map(|access_token| CodexCredentials {
            access_token,
            account_id: codex_account_id_env(),
        })
}

/// Validate only the stored OAuth file, excluding token environment
/// overrides so config-vs-env provenance remains truthful.
#[must_use]
pub fn stored_credentials_present(grant: &ExternalCredentialReadGrant) -> bool {
    load_credentials(grant)
        .ok()
        .flatten()
        .is_some_and(|credentials| !token_is_expired(&credentials.access_token))
}

/// Load read-only credentials from the exact external path authorized by
/// `grant`. Expired tokens fail with guidance; they are never refreshed.
pub fn get_credentials(grant: &ExternalCredentialReadGrant) -> Result<CodexCredentials> {
    let creds = load_credentials(grant)?.with_context(missing_auth_message)?;

    // Check if the access token is still valid.
    if !token_is_expired(&creds.access_token) {
        return Ok(creds);
    }

    bail!(
        "Codex access token in {} is expired. Read-only consent never refreshes or rewrites another CLI's credentials. Run `codex login`, or provide OPENAI_CODEX_ACCESS_TOKEN for this process.",
        codewhale_config::quote_os_path(grant.path())
    )
}

#[must_use]
pub fn missing_auth_message() -> String {
    format!(
        "OpenAI Codex OAuth credentials are unavailable.\n\
         \n\
         Codewhale checks OPENAI_CODEX_ACCESS_TOKEN and CODEX_ACCESS_TOKEN automatically.\n\
         Access to the Codex CLI file is disabled by default. After `codex login`, grant read-only access explicitly with:\n\
         `codewhale auth external-consent --provider openai-codex --mode read-only --path {}`\n\
         Read-only access never refreshes or rewrites the Codex CLI file.",
        codewhale_config::quote_os_path(&auth_file_path())
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(path: &std::path::Path) -> ExternalCredentialReadGrant {
        codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            path.to_path_buf(),
        )
        .read_grant(
            codewhale_config::ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            path,
        )
        .expect("test read grant")
    }

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
    fn credential_presence_rejects_empty_and_malformed_files_without_refresh() {
        let _lock = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("temp Codex home");
        let auth_path = home
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("auth.json");
        let _auth = crate::test_support::EnvVarGuard::set("OPENAI_CODEX_AUTH_FILE", &auth_path);
        let _access = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _legacy_access = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let grant = grant(&auth_path);

        std::fs::write(&auth_path, "{}").expect("empty auth");
        crate::external_credentials::reset_side_effect_trap();
        assert!(!stored_credentials_present(&grant));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );
        assert_eq!(
            crate::external_credentials::complete_side_effect_trap_counts(),
            (1, 1, 0, 0, 0)
        );
        std::fs::write(&auth_path, "{not-json").expect("malformed auth");
        crate::external_credentials::reset_side_effect_trap();
        assert!(!stored_credentials_present(&grant));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );

        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":9999999999}");
        let access_token = format!("header.{payload}.signature");
        std::fs::write(
            &auth_path,
            serde_json::to_vec(&serde_json::json!({
                "tokens": {"access_token": access_token}
            }))
            .expect("valid auth json"),
        )
        .expect("valid auth");
        crate::external_credentials::reset_side_effect_trap();
        assert!(stored_credentials_present(&grant));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );
    }

    #[test]
    fn expired_external_token_fails_without_refresh_or_rewrite() {
        let _lock = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("temp Codex home");
        let auth_path = home
            .path()
            .canonicalize()
            .expect("canonical temp root")
            .join("auth.json");
        let payload = URL_SAFE_NO_PAD.encode(b"{\"exp\":1000000000}");
        let access_token = format!("header.{payload}.signature");
        let raw = serde_json::to_string_pretty(&serde_json::json!({
            "tokens": {
                "access_token": access_token,
                "refresh_token": "must-never-be-used",
                "account_id": "acct-test",
                "future_field": {"preserve": true}
            },
            "future_top_level": [1, 2, 3]
        }))
        .expect("auth fixture");
        std::fs::write(&auth_path, &raw).expect("expired auth fixture");

        crate::external_credentials::reset_side_effect_trap();
        let error = get_credentials(&grant(&auth_path))
            .expect_err("read-only external tokens must not refresh");
        assert!(error.to_string().contains("never refreshes or rewrites"));
        assert_eq!(
            crate::external_credentials::side_effect_trap_counts(),
            (1, 1)
        );
        assert_eq!(
            std::fs::read_to_string(&auth_path).expect("unchanged auth file"),
            raw
        );
    }

    #[test]
    fn auth_file_path_respects_env() {
        // Just verify it returns a path without panicking.
        let path = auth_file_path();
        assert!(path.to_string_lossy().contains("auth.json"));
    }

    #[test]
    fn missing_auth_message_explains_disabled_default_and_explicit_consent() {
        let _lock = crate::test_support::lock_test_env();
        let message = missing_auth_message();

        assert!(message.contains("OpenAI Codex OAuth credentials are unavailable"));
        assert!(message.contains("OPENAI_CODEX_ACCESS_TOKEN"));
        assert!(message.contains("CODEX_ACCESS_TOKEN"));
        assert!(message.contains(&codewhale_config::quote_os_path(&auth_file_path())));
        assert!(message.contains("codex login"));
        assert!(message.contains("external-consent"));
        assert!(message.contains("disabled by default"));
    }

    #[test]
    fn malformed_codex_credential_errors_never_echo_file_contents() {
        let _lock = crate::test_support::lock_test_env();
        let home = tempfile::tempdir().expect("temp Codex home");
        let path = home.path().canonicalize().unwrap().join("auth.json");
        let sentinel = "must-not-appear-in-diagnostics";
        std::fs::write(
            &path,
            format!(r#"{{"tokens":{{"access_token":{{"secret":"{sentinel}"}}}}}}"#),
        )
        .unwrap();

        let error = load_credentials(&grant(&path)).expect_err("malformed schema");
        let message = format!("{error:#}");
        assert!(message.contains("not valid credential JSON"), "{message}");
        assert!(!message.contains(sentinel), "{message}");
    }
}
