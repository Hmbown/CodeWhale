//! Shared secure-storage contract for Codewhale account sessions.
//!
//! The CLI owns device authorization and refresh traffic. This module owns the
//! durable record written by that flow so the TUI and Runtime API can recognize
//! the same account without copying tokens or inventing another login protocol.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{Secrets, SecretsError};

/// Production account API origin used when no override is configured.
pub const DEFAULT_ACCOUNT_API_BASE: &str = "https://api.codewhale.net";
/// Environment variable that selects the account API origin.
pub const ACCOUNT_API_BASE_ENV: &str = "CODEWHALE_CLOUD_API_BASE";
/// Former opt-in for the local file session store. The file store is now the
/// automatic fallback (codex-style); the variable is accepted but ignored.
#[deprecated(
    since = "0.9.11",
    note = "the file session store is the automatic fallback; this variable is ignored"
)]
pub const ACCOUNT_ALLOW_FILE_SESSION_STORE_ENV: &str = "CODEWHALE_CLOUD_ALLOW_FILE_SESSION_STORE";
/// OS credential-manager service shared by CLI, TUI, and Runtime API.
pub const ACCOUNT_KEYRING_SERVICE: &str = "codewhale-cloud";
/// Current serialized account-session record version.
pub const ACCOUNT_SESSION_SCHEMA_VERSION: u8 = 1;

const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_SCOPES: usize = 64;
const MAX_SCOPE_BYTES: usize = 128;

/// A short-lived access credential and its durable refresh/session metadata.
///
/// This type intentionally does not implement `Debug`, preventing accidental
/// token disclosure through ordinary diagnostic formatting.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAuthBundle {
    /// Authentication scheme returned by the account service.
    pub token_type: String,
    /// Short-lived bearer credential. Never serialize this outside secure storage.
    pub access_token: String,
    /// Refresh credential. Never serialize this outside secure storage.
    pub refresh_token: String,
    /// Durable server session metadata, when returned by the service.
    #[serde(default)]
    pub session: Option<AccountSession>,
    /// Cached non-secret account record, when returned by the service.
    #[serde(default)]
    pub user: Option<AccountUser>,
}

/// Durable account-session metadata supplied by the account service.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSession {
    /// Durable session identifier shared across Codewhale surfaces.
    pub id: String,
    /// Authorization provider used to establish the session.
    #[serde(default)]
    pub provider: String,
    /// Linked authorization providers recorded by the account service.
    #[serde(default)]
    pub providers: Vec<String>,
    /// Explicit bounded scopes granted to this session.
    #[serde(default)]
    pub scopes: Vec<String>,
    /// Access-credential expiration in RFC 3339 format.
    #[serde(default)]
    pub expires_at: String,
    /// Refresh/session expiration in RFC 3339 format.
    #[serde(default)]
    pub refresh_expires_at: String,
    /// Explicit server session state when supplied.
    #[serde(default)]
    pub status: String,
    /// Explicit revocation timestamp when supplied.
    #[serde(default)]
    pub revoked_at: String,
}

/// Cached account identity and non-secret presentation fields.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUser {
    /// Stable Codewhale account identifier.
    #[serde(default)]
    pub id: String,
    /// User-facing account name.
    #[serde(default)]
    pub display_name: String,
    /// Account email returned by the service. Runtime metadata never exposes it.
    #[serde(default)]
    pub email: String,
    /// Account residency region returned by the service.
    #[serde(default)]
    pub region: String,
    /// Account plan returned by the service.
    #[serde(default)]
    pub plan: String,
    /// Provider-key presence metadata; values never contain provider credentials.
    #[serde(default)]
    pub model_keys: BTreeMap<String, AccountModelKeyState>,
}

/// Non-secret provider-key presence metadata returned by the account service.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountModelKeyState {
    /// Whether the account service reports a credential for this provider.
    #[serde(default)]
    pub configured: bool,
}

/// Versioned secure-storage envelope shared by every local Codewhale surface.
///
/// This type intentionally does not implement `Debug` because `bundle`
/// contains access and refresh credentials.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredAccountAuth {
    /// Serialized record version.
    pub schema_version: u8,
    /// Exact canonical API origin that owns this session.
    pub api_base: String,
    /// Secret account bundle stored inside the credential manager.
    pub bundle: AccountAuthBundle,
}

/// Normalized account states exposed by the token-free Runtime API contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountSessionState {
    /// No valid secure-store session was found for the selected profile/origin.
    SignedOut,
    /// The cached session and access credential are within their recorded lifetime.
    Authenticated,
    /// Durable identity remains cached, but the access credential has expired.
    OfflineCached,
    /// The durable refresh/session lifetime has ended.
    Expired,
    /// The stored session carries an explicit revocation receipt.
    Revoked,
}

/// Token-free account receipt returned by `GET /v1/runtime/info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeAccountInfo {
    /// Runtime account receipt schema version.
    pub schema_version: u8,
    /// Current account-session state.
    pub state: AccountSessionState,
    /// Exact account API origin used to locate the secure session.
    pub api_base: String,
    /// Stable account identifier, present only when read from secure storage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    /// Durable session identifier, present only when read from secure storage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Explicit session scopes from secure storage; never inferred from identity.
    pub scopes: Vec<String>,
    /// Access-credential expiration, when the stored value is valid RFC 3339.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

impl RuntimeAccountInfo {
    /// Build the fail-closed signed-out receipt for an API origin.
    #[must_use]
    pub fn signed_out(api_base: impl Into<String>) -> Self {
        Self {
            schema_version: ACCOUNT_SESSION_SCHEMA_VERSION,
            state: AccountSessionState::SignedOut,
            api_base: api_base.into(),
            account_id: None,
            session_id: None,
            scopes: Vec::new(),
            expires_at: None,
        }
    }
}

/// Failures while selecting, decoding, or validating account session storage.
#[derive(Debug, Error)]
pub enum AccountSessionError {
    /// Underlying credential-store failure.
    #[error(transparent)]
    Secrets(#[from] SecretsError),
    /// Stored session JSON could not be decoded.
    #[error("the local Codewhale account session is unreadable")]
    UnreadableRecord(#[source] serde_json::Error),
    /// Stored or newly returned authentication credentials are malformed.
    #[error("the Codewhale account session contains invalid credentials")]
    InvalidCredentials,
    /// No approved secure session backend is available.
    #[error(
        "Codewhale account sessions could not open a secret store; check that HOME is writable or set CODEWHALE_HOME to an absolute path"
    )]
    SecureStoreUnavailable,
}

/// Profile- and origin-scoped view of the shared account credential record.
#[derive(Clone)]
pub struct AccountSessionStore {
    secrets: Secrets,
    auth_slot: String,
    api_base: String,
}

impl AccountSessionStore {
    /// Create a store view for one local profile and one validated API origin.
    #[must_use]
    pub fn new(secrets: Secrets, profile: Option<&str>, api_base: &str) -> Self {
        let profile = normalize_account_profile(profile);
        let api_base = api_base.trim().trim_end_matches('/').to_string();
        Self {
            auth_slot: account_auth_slot(&profile, &api_base),
            secrets,
            api_base,
        }
    }

    /// Load and validate the selected account session from secure storage.
    pub fn load(&self) -> Result<Option<StoredAccountAuth>, AccountSessionError> {
        let Some(raw) = self.secrets.get(&self.auth_slot)? else {
            return Ok(None);
        };
        let stored: StoredAccountAuth =
            serde_json::from_str(&raw).map_err(AccountSessionError::UnreadableRecord)?;
        if stored.schema_version != ACCOUNT_SESSION_SCHEMA_VERSION
            || stored.api_base != self.api_base
        {
            return Ok(None);
        }
        validate_account_auth_bundle(&stored.bundle)?;
        Ok(Some(stored))
    }

    /// Validate and save an account bundle in the selected secure-store slot.
    pub fn save(&self, bundle: AccountAuthBundle) -> Result<(), AccountSessionError> {
        validate_account_auth_bundle(&bundle)?;
        let stored = StoredAccountAuth {
            schema_version: ACCOUNT_SESSION_SCHEMA_VERSION,
            api_base: self.api_base.clone(),
            bundle,
        };
        let raw = serde_json::to_string(&stored).map_err(AccountSessionError::UnreadableRecord)?;
        self.secrets.set(&self.auth_slot, &raw)?;
        Ok(())
    }

    /// Remove only the selected profile/origin account session.
    pub fn clear(&self) -> Result<(), AccountSessionError> {
        self.secrets.delete(&self.auth_slot)?;
        Ok(())
    }

    /// Read a token-free runtime receipt at a caller-supplied clock instant.
    pub fn runtime_info_at(
        &self,
        now: DateTime<Utc>,
    ) -> Result<RuntimeAccountInfo, AccountSessionError> {
        let Some(stored) = self.load()? else {
            return Ok(RuntimeAccountInfo::signed_out(self.api_base.clone()));
        };
        Ok(runtime_account_info_from_stored(stored, now))
    }
}

/// Select the approved account-session backend shared by CLI, TUI, and Runtime.
///
/// Account sessions live in the private `0600` Codewhale secrets file. The OS
/// keyring is not used: on macOS every unsigned or rebuilt `codewhale` binary
/// is a new Keychain ACL principal, so reading `codewhale-cloud-auth-v1-*`
/// under the legacy `deepseek` service pops a password dialog on every start.
pub fn secure_account_session_secrets() -> Result<Secrets, AccountSessionError> {
    Ok(Secrets::file_backed())
}

/// Normalize an optional CLI/TUI profile to the durable account slot label.
#[must_use]
pub fn normalize_account_profile(profile: Option<&str>) -> String {
    profile
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string()
}

/// Derive the opaque secure-store slot for a profile and account API origin.
#[must_use]
pub fn account_auth_slot(profile: &str, api_base: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(profile.as_bytes());
    digest.update([0]);
    digest.update(api_base.as_bytes());
    let digest = digest.finalize();
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    format!("codewhale-cloud-auth-v1-{encoded}")
}

/// Validate the credential-bearing portion of an account response or record.
pub fn validate_account_auth_bundle(bundle: &AccountAuthBundle) -> Result<(), AccountSessionError> {
    if !bundle.token_type.eq_ignore_ascii_case("bearer")
        || bundle.access_token.trim().is_empty()
        || bundle.refresh_token.trim().is_empty()
        || bundle.access_token.len() > MAX_TOKEN_BYTES
        || bundle.refresh_token.len() > MAX_TOKEN_BYTES
        || bundle
            .access_token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
        || bundle
            .refresh_token
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(AccountSessionError::InvalidCredentials);
    }
    Ok(())
}

fn runtime_account_info_from_stored(
    stored: StoredAccountAuth,
    now: DateTime<Utc>,
) -> RuntimeAccountInfo {
    let account_id = stored
        .bundle
        .user
        .as_ref()
        .map(|user| user.id.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let session = stored.bundle.session.as_ref();
    let session_id = session
        .map(|session| session.id.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expires_at = session
        .map(|session| session.expires_at.trim())
        .filter(|value| parse_rfc3339(value).is_some())
        .map(str::to_string);
    let scopes = normalized_scopes(session.map_or(&[], |session| &session.scopes));
    let state = session.map_or(AccountSessionState::OfflineCached, |session| {
        classify_session_state(session, now)
    });
    RuntimeAccountInfo {
        schema_version: ACCOUNT_SESSION_SCHEMA_VERSION,
        state,
        api_base: stored.api_base,
        account_id,
        session_id,
        scopes,
        expires_at,
    }
}

fn classify_session_state(session: &AccountSession, now: DateTime<Utc>) -> AccountSessionState {
    let explicit = session.status.trim().to_ascii_lowercase();
    if explicit == "revoked" || !session.revoked_at.trim().is_empty() {
        return AccountSessionState::Revoked;
    }
    if explicit == "expired"
        || parse_rfc3339(&session.refresh_expires_at).is_some_and(|expiry| expiry <= now)
    {
        return AccountSessionState::Expired;
    }
    if explicit == "offline_cached"
        || parse_rfc3339(&session.expires_at).is_some_and(|expiry| expiry <= now)
    {
        return AccountSessionState::OfflineCached;
    }
    AccountSessionState::Authenticated
}

fn parse_rfc3339(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.trim())
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn normalized_scopes(scopes: &[String]) -> Vec<String> {
    let mut scopes = scopes
        .iter()
        .map(|scope| scope.trim())
        .filter(|scope| {
            !scope.is_empty()
                && scope.len() <= MAX_SCOPE_BYTES
                && scope.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'_' | b'-' | b'/')
                })
        })
        .map(str::to_string)
        .collect::<Vec<_>>();
    scopes.sort();
    scopes.dedup();
    scopes.truncate(MAX_SCOPES);
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::InMemoryKeyringStore;

    /// Account sessions must never touch the OS keyring. Unsigned rebuilds
    /// on macOS otherwise prompt on every `codewhale web` / TUI start.
    #[test]
    fn session_secrets_use_the_file_store_not_keychain() {
        let _lock = crate::tests::env_lock();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let home = dir.path().join("codewhale-home");
        std::fs::create_dir_all(&home).expect("home");
        unsafe { std::env::set_var("CODEWHALE_HOME", &home) };
        unsafe { std::env::remove_var("CODEWHALE_CLOUD_ALLOW_FILE_SESSION_STORE") };
        unsafe { std::env::remove_var("CODEWHALE_SECRET_BACKEND") };
        unsafe { std::env::remove_var("DEEPSEEK_SECRET_BACKEND") };
        let secrets =
            secure_account_session_secrets().expect("session secrets must resolve without opt-in");
        let name = secrets.backend_name();
        assert!(
            name.to_lowercase().contains("file"),
            "account sessions must not use Keychain: {name}"
        );
        assert!(!name.to_lowercase().contains("keychain"));
        assert!(!name.to_lowercase().contains("keyring"));
    }

    fn auth(
        account_id: &str,
        session_id: &str,
        expires_at: &str,
        refresh_expires_at: &str,
    ) -> AccountAuthBundle {
        AccountAuthBundle {
            token_type: "Bearer".to_string(),
            access_token: "access-never-serialize".to_string(),
            refresh_token: "refresh-never-serialize".to_string(),
            session: Some(AccountSession {
                id: session_id.to_string(),
                scopes: vec!["identity:read".to_string(), "session:sync".to_string()],
                expires_at: expires_at.to_string(),
                refresh_expires_at: refresh_expires_at.to_string(),
                ..AccountSession::default()
            }),
            user: Some(AccountUser {
                id: account_id.to_string(),
                email: "private@example.test".to_string(),
                ..AccountUser::default()
            }),
        }
    }

    fn test_store() -> (Secrets, Arc<InMemoryKeyringStore>) {
        let store = Arc::new(InMemoryKeyringStore::new());
        (Secrets::new(store.clone()), store)
    }

    #[test]
    fn runtime_receipt_is_token_free_and_preserves_only_explicit_scopes() {
        let (secrets, _) = test_store();
        let store = AccountSessionStore::new(secrets, Some("work"), "https://api.codewhale.net");
        store
            .save(auth(
                "acct-1",
                "session-1",
                "2030-01-01T00:00:00Z",
                "2031-01-01T00:00:00Z",
            ))
            .unwrap();

        let info = store.runtime_info_at(Utc::now()).unwrap();
        assert_eq!(info.state, AccountSessionState::Authenticated);
        assert_eq!(info.account_id.as_deref(), Some("acct-1"));
        assert_eq!(info.session_id.as_deref(), Some("session-1"));
        assert_eq!(info.scopes, ["identity:read", "session:sync"]);
        let json = serde_json::to_string(&info).unwrap();
        for secret in [
            "access-never-serialize",
            "refresh-never-serialize",
            "private@example.test",
        ] {
            assert!(!json.contains(secret));
        }
    }

    #[test]
    fn profiles_and_origins_preserve_same_account_and_cross_account_isolation() {
        let (secrets, _) = test_store();
        let default = AccountSessionStore::new(secrets.clone(), None, "https://api.codewhale.net");
        let work =
            AccountSessionStore::new(secrets.clone(), Some("work"), "https://api.codewhale.net");
        let local = AccountSessionStore::new(secrets, None, "http://127.0.0.1:8787");
        default.save(auth("acct-a", "session-a", "", "")).unwrap();
        work.save(auth("acct-a", "session-b", "", "")).unwrap();
        local.save(auth("acct-b", "session-c", "", "")).unwrap();

        let now = Utc::now();
        assert_eq!(
            default.runtime_info_at(now).unwrap().account_id.as_deref(),
            Some("acct-a")
        );
        assert_eq!(
            work.runtime_info_at(now).unwrap().account_id.as_deref(),
            Some("acct-a")
        );
        assert_eq!(
            local.runtime_info_at(now).unwrap().account_id.as_deref(),
            Some("acct-b")
        );
        default.clear().unwrap();
        assert_eq!(
            default.runtime_info_at(now).unwrap().state,
            AccountSessionState::SignedOut
        );
        assert_eq!(
            work.runtime_info_at(now).unwrap().state,
            AccountSessionState::Authenticated
        );
    }

    #[test]
    fn signed_out_expired_offline_and_revoked_states_are_distinct() {
        let (secrets, _) = test_store();
        let store = AccountSessionStore::new(secrets, None, DEFAULT_ACCOUNT_API_BASE);
        let now = DateTime::parse_from_rfc3339("2029-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            store.runtime_info_at(now).unwrap().state,
            AccountSessionState::SignedOut
        );

        store
            .save(auth(
                "acct",
                "offline",
                "2028-12-31T23:59:59Z",
                "2029-12-31T23:59:59Z",
            ))
            .unwrap();
        assert_eq!(
            store.runtime_info_at(now).unwrap().state,
            AccountSessionState::OfflineCached
        );

        store
            .save(auth(
                "acct",
                "expired",
                "2028-12-31T23:59:59Z",
                "2028-12-31T23:59:59Z",
            ))
            .unwrap();
        assert_eq!(
            store.runtime_info_at(now).unwrap().state,
            AccountSessionState::Expired
        );

        let mut revoked = auth("acct", "revoked", "2030-01-01T00:00:00Z", "");
        revoked.session.as_mut().unwrap().status = "revoked".to_string();
        store.save(revoked).unwrap();
        assert_eq!(
            store.runtime_info_at(now).unwrap().state,
            AccountSessionState::Revoked
        );
    }
}
