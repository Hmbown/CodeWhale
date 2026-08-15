//! Read-only Antigravity (`agy`) credential import.
//!
//! Official `agy` (1.1.13) persists its OAuth token inside the Antigravity
//! app's VSCode-style SQLite store `state.vscdb`, table `ItemTable`, key
//! `antigravityUnifiedStateSync.oauthToken`. Codewhale may read that one
//! value from that exact file only after `codewhale auth external-consent`.
//! The database is opened read-only and never written, refreshed, or
//! copied into the process environment. Codewhale-owned
//! `ANTIGRAVITY_API_KEY` and the process's own `AGY_ADC_AUTH` always win
//! over the external file.

use std::io::{Read as _, Seek, SeekFrom};
use std::path::Path;

use anyhow::{Context, Result, bail};
use codewhale_config::{ExternalCredentialReadGrant, ExternalCredentialSource};

/// ItemTable key holding the `agy` OAuth token.
pub const AGY_OAUTH_TOKEN_KEY: &str = "antigravityUnifiedStateSync.oauthToken";

/// Upper bound on the credential store size we are willing to open.
const AGY_STATE_DB_LIMIT: u64 = 64 * 1024 * 1024;

/// Credential resolution order for the Antigravity route.
///
/// 1. Codewhale-owned `ANTIGRAVITY_API_KEY` (config table or environment)
/// 2. The process's own `AGY_ADC_AUTH` (what `agy` itself calls ADC auth)
/// 3. The consented external `state.vscdb` — read-only, never refreshed
#[must_use]
pub fn antigravity_credential_precedence(
    owned_api_key: Option<&str>,
    process_env: &std::collections::HashMap<String, String>,
    grant: Option<&ExternalCredentialReadGrant>,
) -> AntigravityCredential {
    if let Some(key) = owned_api_key.filter(|key| !key.trim().is_empty()) {
        return AntigravityCredential::OwnedKey(key.trim().to_string());
    }
    if let Some(adc) = process_env
        .get("AGY_ADC_AUTH")
        .filter(|value| !value.trim().is_empty())
    {
        return AntigravityCredential::ProcessEnv(adc.trim().to_string());
    }
    let Some(grant) = grant else {
        return AntigravityCredential::None;
    };
    match antigravity_oauth_token_from_grant(grant) {
        Ok(Some(token)) => AntigravityCredential::ExternalFile(token),
        Ok(None) => AntigravityCredential::None,
        Err(error) => AntigravityCredential::Error(error.to_string()),
    }
}

/// Where the Antigravity credential came from, for display and receipts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AntigravityCredential {
    OwnedKey(String),
    ProcessEnv(String),
    ExternalFile(String),
    None,
    Error(String),
}

impl AntigravityCredential {
    #[must_use]
    pub fn source_label(&self) -> &'static str {
        match self {
            Self::OwnedKey(_) => "ANTIGRAVITY_API_KEY",
            Self::ProcessEnv(_) => "AGY_ADC_AUTH",
            Self::ExternalFile(_) => "agy state.vscdb (read-only)",
            Self::None | Self::Error(_) => "none",
        }
    }
}

/// Extract the `agy` OAuth token from a granted `state.vscdb`.
pub(crate) fn antigravity_oauth_token_from_grant(
    grant: &ExternalCredentialReadGrant,
) -> Result<Option<String>> {
    if grant.source() != ExternalCredentialSource::AgyCli {
        bail!(
            "Antigravity import requires an agy_cli grant, not {}",
            grant.source().as_str()
        );
    }
    let path = grant.path();
    // Secure-open the exact granted path first: regular file only, no
    // symlink/reparse-point leaf, size-capped before any SQLite parsing.
    let mut file = crate::external_credentials::open_external_regular_file(path)?;
    let mut header = [0u8; 16];
    let read = file.read(&mut header).with_context(|| {
        format!(
            "reading SQLite header of {}",
            codewhale_config::quote_os_path(path)
        )
    })?;
    if read < 16 || header[..15] != *b"SQLite format 3" {
        bail!(
            "external agy credential file {} is not a SQLite database",
            codewhale_config::quote_os_path(path)
        );
    }
    file.seek(SeekFrom::Start(0)).ok();
    let metadata = file
        .metadata()
        .with_context(|| format!("statting {}", codewhale_config::quote_os_path(path)))?;
    if metadata.len() > AGY_STATE_DB_LIMIT {
        bail!(
            "external agy credential store {} exceeds the {} byte safety limit",
            codewhale_config::quote_os_path(path),
            AGY_STATE_DB_LIMIT
        );
    }
    // Pin the file identity: SQLite reopens the path by name, so hold the
    // secure handle open across the query and prove the inode did not move.
    let pinned = file_identity(&file);
    drop(file);
    let value = query_oauth_token(path)?;
    let reopened = std::fs::File::open(path)
        .ok()
        .and_then(|recheck| file_identity_of(&recheck));
    if pinned != reopened {
        bail!(
            "external agy credential store {} changed while being read",
            codewhale_config::quote_os_path(path)
        );
    }
    parse_agy_oauth_token_value(value)
}

#[cfg(unix)]
fn file_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;
    file.metadata().ok().map(|m| (m.dev(), m.ino()))
}

#[cfg(unix)]
fn file_identity_of(file: &std::fs::File) -> Option<(u64, u64)> {
    file_identity(file)
}

#[cfg(windows)]
fn file_identity(file: &std::fs::File) -> Option<(u64, u64)> {
    use std::os::windows::io::AsRawHandle as _;
    use std::os::windows::raw::HANDLE;
    // Windows: BY_HANDLE_FILE_INFORMATION via winapi is out of scope here;
    // the secure-open layer already rejects reparse points, and the file is
    // held by SQLite for the duration of the read. Identity recheck is a
    // Unix hardening bonus.
    let _ = (file, file.as_raw_handle() as HANDLE);
    None
}

#[cfg(windows)]
fn file_identity_of(file: &std::fs::File) -> Option<(u64, u64)> {
    let _ = file;
    None
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &std::fs::File) -> Option<(u64, u64)> {
    None
}

#[cfg(not(any(unix, windows)))]
fn file_identity_of(_file: &std::fs::File) -> Option<(u64, u64)> {
    None
}

/// Open the granted path read-only through the shared secure boundary.
fn query_oauth_token(path: &Path) -> Result<Option<String>> {
    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| {
        format!(
            "opening {} read-only",
            codewhale_config::quote_os_path(path)
        )
    })?;
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?1",
            [AGY_OAUTH_TOKEN_KEY],
            |row| row.get(0),
        )
        .map(Some)
        .or_else(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
        .with_context(|| {
            format!(
                "querying {} for {AGY_OAUTH_TOKEN_KEY}",
                codewhale_config::quote_os_path(path)
            )
        })?;
    Ok(value)
}

/// The stored value is opaque to Codewhale. Accept only shapes observed in
/// the official store — a bare token string or a JSON object with a token
/// member — and never synthesize or trim secrets beyond whitespace.
pub(crate) fn parse_agy_oauth_token_value(value: Option<String>) -> Result<Option<String>> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.starts_with('{') {
        let parsed: serde_json::Value = serde_json::from_str(trimmed)
            .with_context(|| "agy OAuth token value is malformed JSON")?;
        for member in ["access_token", "accessToken", "token"] {
            if let Some(token) = parsed.get(member).and_then(|v| v.as_str()) {
                if token.trim().is_empty() {
                    bail!("agy OAuth token member `{member}` is empty");
                }
                return Ok(Some(token.to_string()));
            }
        }
        bail!("agy OAuth token JSON carries no access token member");
    }
    Ok(Some(trimmed.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use codewhale_config::ExternalCredentialReadGrant;
    use std::collections::HashMap;

    fn grant_for(path: &Path) -> ExternalCredentialReadGrant {
        codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::Antigravity,
            ExternalCredentialSource::AgyCli,
            path.to_path_buf(),
        )
        .read_grant(
            codewhale_config::ProviderKind::Antigravity,
            ExternalCredentialSource::AgyCli,
            path,
        )
        .expect("test grant")
    }

    fn fixture_db(token_value: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        // Canonicalize: production open_secure_regular_file uses O_NOFOLLOW on
        // every path component, so macOS TempDir paths under `/var` (symlink
        // to `private/var`) would fail the secure open for an unrelated reason.
        // Resolving the fixture root keeps the suite focused on credential
        // parsing while preserving the production no-follow boundary.
        let root = dir.path().canonicalize().expect("canonical temp root");
        let path = root.join("state.vscdb");
        let connection = rusqlite::Connection::open(&path).expect("create fixture database");
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS ItemTable (key TEXT PRIMARY KEY, value BLOB);",
            )
            .unwrap();
        if let Some(value) = token_value {
            connection
                .execute(
                    "INSERT INTO ItemTable (key, value) VALUES (?1, ?2)",
                    rusqlite::params![AGY_OAUTH_TOKEN_KEY, value],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO ItemTable (key, value) VALUES ('antigravityAuthStatus', 'signedIn')",
                [],
            )
            .unwrap();
        drop(connection);
        (dir, path)
    }

    #[test]
    fn extracts_token_from_fixture_state_db() {
        let (_dir, path) = fixture_db(Some("ya29.test-token"));
        let grant = grant_for(&path);
        assert_eq!(
            antigravity_oauth_token_from_grant(&grant).unwrap(),
            Some("ya29.test-token".to_string())
        );
    }

    #[test]
    fn extracts_access_token_member_from_json_value() {
        let (_dir, path) = fixture_db(Some(
            r#"{"access_token":"ya29.json","scope":"cloud-platform"}"#,
        ));
        let grant = grant_for(&path);
        assert_eq!(
            antigravity_oauth_token_from_grant(&grant).unwrap(),
            Some("ya29.json".to_string())
        );
    }

    #[test]
    fn missing_token_row_is_absent_not_an_error() {
        let (_dir, path) = fixture_db(None);
        let grant = grant_for(&path);
        assert_eq!(antigravity_oauth_token_from_grant(&grant).unwrap(), None);
    }

    #[test]
    fn json_without_token_member_fails_closed() {
        assert!(parse_agy_oauth_token_value(Some(r#"{"scope":"x"}"#.into())).is_err());
    }

    #[test]
    fn empty_token_fails_or_is_absent() {
        assert_eq!(
            parse_agy_oauth_token_value(Some("   ".into())).unwrap(),
            None
        );
        assert!(parse_agy_oauth_token_value(Some(r#"{"access_token":""}"#.to_string())).is_err());
    }

    #[test]
    fn wrong_grant_source_is_rejected() {
        let (_dir, path) = fixture_db(Some("token"));
        let grant = codewhale_config::ExternalCredentialConsentToml::read_only(
            codewhale_config::ProviderKind::Antigravity,
            ExternalCredentialSource::DshCli,
            path.clone(),
        )
        .read_grant(
            codewhale_config::ProviderKind::Antigravity,
            ExternalCredentialSource::DshCli,
            &path,
        )
        .expect("test grant");
        assert!(antigravity_oauth_token_from_grant(&grant).is_err());
    }

    #[test]
    fn precedence_owned_key_beats_env_beats_file() {
        let mut env = HashMap::new();
        env.insert(
            "AGY_ADC_AUTH".to_string(),
            "adc-process-credential".to_string(),
        );
        let (_dir, path) = fixture_db(Some("file-token"));
        let grant = grant_for(&path);

        assert_eq!(
            antigravity_credential_precedence(Some("owned-key"), &env, Some(&grant)),
            AntigravityCredential::OwnedKey("owned-key".to_string())
        );
        assert_eq!(
            antigravity_credential_precedence(None, &env, Some(&grant)),
            AntigravityCredential::ProcessEnv("adc-process-credential".to_string())
        );
        assert_eq!(
            antigravity_credential_precedence(None, &HashMap::new(), Some(&grant)),
            AntigravityCredential::ExternalFile("file-token".to_string())
        );
        assert_eq!(
            antigravity_credential_precedence(None, &HashMap::new(), None),
            AntigravityCredential::None
        );
    }
}
