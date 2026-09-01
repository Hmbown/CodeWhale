//! Secret storage for CodeWhale API keys.
//!
//! Provides a small abstraction (`KeyringStore`) plus a default
//! file-based implementation (`FileKeyringStore`), an opt-in OS keyring
//! implementation (`DefaultKeyringStore`), and an in-memory store for tests
//! (`InMemoryKeyringStore`).
//!
//! Higher-level lookup through [`Secrets::resolve`] checks the secret store first
//! and falls back to environment variables. Config-file precedence lives in the
//! config crate so user-facing commands can keep `config -> secret store -> env`
//! explicit at the call site.
#![deny(missing_docs)]

/// Shared secure-storage contract for the Codewhale account session.
pub mod account;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use codewhale_paths::codewhale_home_is_explicit;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default OS keychain service name. Kept as `deepseek` for compatibility
/// with credentials saved before the CodeWhale rename. macOS users can verify
/// entries with `security find-generic-password -s deepseek -a <provider>`.
pub const DEFAULT_SERVICE: &str = "deepseek";
/// Secret-store slot consumed by Daytona cloud dispatch (`codewhale dispatch`).
///
/// Login writes here; dispatch looks this slot up after `DAYTONA_API_KEY` and
/// `CWC_DAYTONA_TOKEN`. Do not invent a second Daytona credential name.
pub const DAYTONA_TOKEN_SLOT: &str = "daytona";
/// First-class Daytona process env that dispatch also accepts.
pub const DAYTONA_API_KEY_ENV: &str = "DAYTONA_API_KEY";
/// CWC alias that Daytona dispatch also accepts.
pub const CWC_DAYTONA_TOKEN_ENV: &str = "CWC_DAYTONA_TOKEN";
/// Select the secret storage backend. Supported values are `file` (default)
/// and `system`/`keyring` for the OS credential store.
pub const SECRET_BACKEND_ENV: &str = "CODEWHALE_SECRET_BACKEND";
/// Legacy alias for [`SECRET_BACKEND_ENV`].
pub const LEGACY_SECRET_BACKEND_ENV: &str = "DEEPSEEK_SECRET_BACKEND";
const FILE_BACKEND_LABEL: &str = "file-based (~/.codewhale/secrets/)";

/// Errors that may arise from a [`KeyringStore`] backend.
#[derive(Debug, Error)]
pub enum SecretsError {
    /// Underlying OS keyring backend reported an error.
    #[error("keyring backend error: {0}")]
    Keyring(String),
    /// File-backed fallback I/O error.
    #[error("file-backed secret store I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// File-backed fallback JSON (de)serialisation error.
    #[error("file-backed secret store JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// Caught when a stored secret on disk has unsafe permissions.
    #[error("file-backed secret store at {path} has insecure permissions {mode:o} (expected 0600)")]
    InsecurePermissions {
        /// Absolute path to the secrets file.
        path: PathBuf,
        /// Observed unix permission mode.
        mode: u32,
    },
    /// A caller attempted to modify a diagnostic-only secret store.
    #[error("secret store is read-only")]
    ReadOnly,
}

/// Abstract secret store trait.
///
/// Concrete implementations may use the OS keyring ([`DefaultKeyringStore`]),
/// a JSON file under `~/.codewhale/secrets/` ([`FileKeyringStore`]), or an
/// in-memory map for tests ([`InMemoryKeyringStore`]).
///
/// All implementations must be [`Send`] + [`Sync`] so they can be shared
/// across threads via [`Arc`].
pub trait KeyringStore: Send + Sync {
    /// Read a secret by key.
    ///
    /// Returns `Ok(None)` if no entry exists for the given key. Returns
    /// `Err` only on backend failures (I/O errors, keyring access issues).
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError>;

    /// Write a secret, replacing any existing value for the same key.
    ///
    /// Creates the backing store (e.g. the JSON file) on first write if
    /// it does not yet exist.
    fn set(&self, key: &str, value: &str) -> Result<(), SecretsError>;

    /// Remove a secret by key.
    ///
    /// Implementations should succeed (no-op) if the entry is already absent
    /// rather than returning an error.
    fn delete(&self, key: &str) -> Result<(), SecretsError>;

    /// Short, human-readable label for this backend.
    ///
    /// Used by diagnostic output (e.g. `doctor` command) to indicate which
    /// storage backend is active. Examples: `"file-based (~/.codewhale/secrets/)"`,
    /// `"system keyring"`, `"in-memory (test)"`.
    fn backend_name(&self) -> &'static str;
}

/// OS-native keyring backend.
///
/// Wraps the platform credential store:
/// - **macOS**: Keychain (via `security` framework)
/// - **Windows**: Credential Manager
/// - **Linux**: Secret Service (GNOME Keyring / kwallet via dbus), excluding OHOS
///
/// This backend is opt-in -- set the [`SECRET_BACKEND_ENV`] environment
/// variable to `system` or `keyring` to activate it. On platforms without
/// a configured native keyring dependency, [`probe`](DefaultKeyringStore::probe)
/// returns an unsupported error so [`Secrets::auto_detect`] can transparently
/// fall back to [`FileKeyringStore`].
#[derive(Debug, Clone)]
pub struct DefaultKeyringStore {
    /// Keyring service name used to namespace stored credentials.
    /// Defaults to [`DEFAULT_SERVICE`].
    service: String,
}

impl Default for DefaultKeyringStore {
    fn default() -> Self {
        Self::new(DEFAULT_SERVICE)
    }
}

impl DefaultKeyringStore {
    /// Build a new store with the given service name.
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    /// Probe the OS keyring without writing anything. Returns `Ok(())` if
    /// a backend is reachable, otherwise an error describing why not.
    ///
    /// The probe reads a deliberately-nonexistent entry: reaching the
    /// backend and learning the entry is absent *is* the reachability
    /// signal. This does not prompt on any supported platform — macOS only
    /// surfaces Keychain UI when accessing an *existing* item owned by
    /// another application, and Windows Credential Manager never prompts
    /// for a missing target — so `__probe__` under our own service name is
    /// safe to read. `Entry::new` alone validates only argument shapes,
    /// which left this probe a no-op on macOS/Windows and the documented
    /// file-store fallback unreachable there (#5172).
    pub fn probe(&self) -> Result<(), SecretsError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        ))]
        {
            let entry = keyring::Entry::new(&self.service, "__probe__")
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            match entry.get_password() {
                Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(keyring::Error::PlatformFailure(err)) => {
                    Err(SecretsError::Keyring(format!("platform failure: {err}")))
                }
                Err(keyring::Error::NoStorageAccess(err)) => {
                    Err(SecretsError::Keyring(format!("no storage access: {err}")))
                }
                Err(other) => Err(SecretsError::Keyring(other.to_string())),
            }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        )))]
        {
            let _ = &self.service;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }
}

impl KeyringStore for DefaultKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        ))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(SecretsError::Keyring(err.to_string())),
            }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        )))]
        {
            let _ = key;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretsError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        ))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            entry
                .set_password(value)
                .map_err(|err| SecretsError::Keyring(err.to_string()))
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        )))]
        {
            let _ = (key, value);
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        #[cfg(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        ))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(SecretsError::Keyring(err.to_string())),
            }
        }
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            all(
                target_os = "linux",
                not(target_env = "ohos"),
                not(target_env = "musl")
            )
        )))]
        {
            let _ = key;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn backend_name(&self) -> &'static str {
        "system keyring"
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "windows",
    all(
        target_os = "linux",
        not(target_env = "ohos"),
        not(target_env = "musl")
    )
)))]
fn unsupported_keyring_message() -> String {
    "system keyring backend is unsupported on this platform".to_string()
}

/// In-memory keyring store for tests.
///
/// Stores secrets in a [`HashMap`] protected by a [`Mutex`]. Not persisted
/// to disk -- all entries are lost when the process exits. This is the
/// preferred store for unit tests because it requires no filesystem setup
/// and is safe to use in parallel test threads.
#[derive(Debug, Default)]
pub struct InMemoryKeyringStore {
    /// Thread-safe map of key-value pairs.
    entries: Mutex<HashMap<String, String>>,
}

impl InMemoryKeyringStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyringStore for InMemoryKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        let guard = self.entries.lock().map_err(|e| {
            SecretsError::Keyring(format!("InMemoryKeyringStore mutex poisoned: {e}"))
        })?;
        Ok(guard.get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretsError> {
        let mut guard = self.entries.lock().map_err(|e| {
            SecretsError::Keyring(format!("InMemoryKeyringStore mutex poisoned: {e}"))
        })?;
        guard.insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        let mut guard = self.entries.lock().map_err(|e| {
            SecretsError::Keyring(format!("InMemoryKeyringStore mutex poisoned: {e}"))
        })?;
        guard.remove(key);
        Ok(())
    }

    fn backend_name(&self) -> &'static str {
        "in-memory (test)"
    }
}

/// JSON-on-disk secret store for headless environments.
///
/// This is the default backend. Secrets are serialised as a JSON object
/// at `<home>/.codewhale/secrets/secrets.json` with Unix file mode `0600`
/// (owner read/write only). The parent directory is created with mode `0700`
/// if it does not exist.
///
/// On Unix, the store rejects files whose permissions are more permissive
/// than `0600` (i.e. group or world bits are set). This prevents other
/// users on the system from reading stored credentials. On Windows, the
/// ACL model is too different to enforce programmatically; callers are
/// responsible for placing the file in a per-user directory.
#[derive(Debug, Clone)]
pub struct FileKeyringStore {
    /// Absolute path to the JSON secrets file.
    path: PathBuf,
}

/// File-backed secret lookup that never migrates or changes either store.
///
/// Normal runtime credential resolution keeps its additive legacy migration:
/// older entries under `~/.deepseek/secrets/` are copied into the Codewhale
/// location before use. Diagnostic commands need the same read precedence
/// without creating that destination, so this store reads the primary file
/// first and falls back to the legacy file only when the primary has no entry
/// and the Codewhale home is not explicitly isolated.
#[derive(Debug, Clone)]
struct ReadOnlyFileKeyringStore {
    primary: FileKeyringStore,
    /// The ambient legacy store is unavailable when `CODEWHALE_HOME` is an
    /// explicit isolation boundary.
    legacy: Option<FileKeyringStore>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileSecretsBlob {
    #[serde(default)]
    entries: HashMap<String, String>,
}

impl FileKeyringStore {
    /// Build a store backed by the given JSON file path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Default path: `<home>/.codewhale/secrets/secrets.json`. Honours
    /// `CODEWHALE_HOME`, then `HOME`, `USERPROFILE`, and finally the platform
    /// home directory from the `dirs` crate. On first use, non-conflicting
    /// entries from the legacy `<home>/.deepseek/secrets/secrets.json` file are
    /// copied into the CodeWhale store — unless `CODEWHALE_HOME` is explicit,
    /// in which case ambient `$HOME/.deepseek` credentials are never imported.
    pub fn default_path() -> Result<PathBuf, SecretsError> {
        let primary = default_codewhale_secrets_path()?;
        // Match the diagnostic isolation boundary: an explicit Codewhale home
        // must not silently pull ambient legacy DeepSeek credentials.
        if !codewhale_home_is_explicit() {
            match legacy_deepseek_secrets_path() {
                Ok(legacy) => {
                    if let Err(err) = Self::migrate_legacy_file_if_needed(&primary, &legacy) {
                        tracing::warn!(
                            "could not migrate legacy secret store from {} to {}: {err}",
                            legacy.display(),
                            primary.display()
                        );
                    }
                }
                Err(err) => {
                    tracing::warn!("could not resolve legacy secret store path: {err}");
                }
            }
        }
        Ok(primary)
    }

    /// Resolve the primary and legacy secret paths without performing legacy
    /// migration.
    ///
    /// This is intended for diagnostic-only lookup. Runtime and authentication
    /// flows must keep using [`Self::default_path`] so their existing additive
    /// migration behavior remains unchanged.
    pub fn default_paths_read_only() -> Result<(PathBuf, Option<PathBuf>), SecretsError> {
        let primary = default_codewhale_secrets_path()?;
        let legacy = (!codewhale_home_is_explicit())
            .then(legacy_deepseek_secrets_path)
            .transpose()?;
        Ok((primary, legacy))
    }

    fn migrate_legacy_file_if_needed(primary: &Path, legacy: &Path) -> Result<(), SecretsError> {
        if !legacy.exists() {
            return Ok(());
        }

        let legacy_store = Self::new(legacy.to_path_buf());
        let legacy_blob = legacy_store.load_unlocked()?;
        if legacy_blob.entries.is_empty() {
            return Ok(());
        }

        let primary_store = Self::new(primary.to_path_buf());
        let mut primary_blob = primary_store.load_unlocked()?;
        let mut changed = false;
        for (key, value) in legacy_blob.entries {
            if let std::collections::hash_map::Entry::Vacant(entry) =
                primary_blob.entries.entry(key)
            {
                entry.insert(value);
                changed = true;
            }
        }
        if changed {
            primary_store.store_unlocked(&primary_blob)?;
        }
        Ok(())
    }

    /// Path used for storage.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load_unlocked(&self) -> Result<FileSecretsBlob, SecretsError> {
        if !self.path.exists() {
            return Ok(FileSecretsBlob::default());
        }
        // Reject files with unsafe permissions on unix. On Windows the
        // ACL model is too different to enforce here; the caller is
        // responsible for placing the file in a per-user directory.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(&self.path)?;
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                return Err(SecretsError::InsecurePermissions {
                    path: self.path.clone(),
                    mode,
                });
            }
        }
        let raw = fs::read_to_string(&self.path)?;
        if raw.trim().is_empty() {
            return Ok(FileSecretsBlob::default());
        }
        let blob: FileSecretsBlob = serde_json::from_str(&raw)?;
        Ok(blob)
    }

    fn store_unlocked(&self, blob: &FileSecretsBlob) -> Result<(), SecretsError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(parent)?.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(parent, perms);
            }
        }
        let body = serde_json::to_string_pretty(blob)?;
        write_private_file(&self.path, body.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Best-effort 0o600 — matches the parent-dir chmod above which
            // is also `let _ = ...`. Filesystems that don't support Unix
            // chmod (Docker bind-mounts of NTFS, network shares — #897)
            // would otherwise fail the whole save here even though the
            // blob already wrote successfully. The host's native ACLs
            // are doing access control in those environments.
            if let Ok(meta) = fs::metadata(&self.path) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&self.path, perms);
            }
        }
        Ok(())
    }
}

impl ReadOnlyFileKeyringStore {
    fn default_for_diagnostics() -> Result<Self, SecretsError> {
        let (primary, legacy) = FileKeyringStore::default_paths_read_only()?;
        Ok(Self::new(primary, legacy))
    }

    fn new(primary: impl Into<PathBuf>, legacy: Option<PathBuf>) -> Self {
        Self {
            primary: FileKeyringStore::new(primary),
            legacy: legacy.map(FileKeyringStore::new),
        }
    }
}

impl KeyringStore for ReadOnlyFileKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        match self.primary.get(key)? {
            Some(value) => Ok(Some(value)),
            None => self
                .legacy
                .as_ref()
                .map_or(Ok(None), |legacy| legacy.get(key)),
        }
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), SecretsError> {
        Err(SecretsError::ReadOnly)
    }

    fn delete(&self, _key: &str) -> Result<(), SecretsError> {
        Err(SecretsError::ReadOnly)
    }

    fn backend_name(&self) -> &'static str {
        FILE_BACKEND_LABEL
    }
}

#[derive(Clone)]
struct ReadOnlyKeyringStore {
    inner: Arc<dyn KeyringStore>,
}

impl ReadOnlyKeyringStore {
    fn new(inner: Arc<dyn KeyringStore>) -> Self {
        Self { inner }
    }
}

impl KeyringStore for ReadOnlyKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        self.inner.get(key)
    }

    fn set(&self, _key: &str, _value: &str) -> Result<(), SecretsError> {
        Err(SecretsError::ReadOnly)
    }

    fn delete(&self, _key: &str) -> Result<(), SecretsError> {
        Err(SecretsError::ReadOnly)
    }

    fn backend_name(&self) -> &'static str {
        self.inner.backend_name()
    }
}

fn write_private_file(path: &Path, body: &[u8]) -> Result<(), SecretsError> {
    atomic_write_private_file(path, body)
}

fn atomic_write_private_file(path: &Path, body: &[u8]) -> Result<(), SecretsError> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(SecretsError::Io)?;
    use std::io::Write as _;
    tmp.write_all(body).map_err(SecretsError::Io)?;
    tmp.flush().map_err(SecretsError::Io)?;
    tmp.as_file().sync_all().map_err(SecretsError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o600);
        tmp.as_file()
            .set_permissions(perms)
            .map_err(SecretsError::Io)?;
    }
    tmp.persist(path).map_err(|e| SecretsError::Io(e.error))?;
    Ok(())
}

impl KeyringStore for FileKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        let blob = self.load_unlocked()?;
        Ok(blob.entries.get(key).cloned())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretsError> {
        // load_unlocked already returns Ok(default) for a missing file, so the
        // first-write-creates-the-file path is preserved. Any other Err
        // (insecure permissions, corrupt JSON, transient I/O) MUST surface to
        // the caller — propagating it via `unwrap_or_default()` silently
        // wipes every previously stored secret on the next `store_unlocked`.
        let mut blob = self.load_unlocked()?;
        blob.entries.insert(key.to_string(), value.to_string());
        self.store_unlocked(&blob)
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        // Same invariant as `set`: never fall back to an empty blob on read
        // error, or `delete <one-key>` becomes `delete <every-key>`.
        let mut blob = self.load_unlocked()?;
        blob.entries.remove(key);
        self.store_unlocked(&blob)
    }

    fn backend_name(&self) -> &'static str {
        FILE_BACKEND_LABEL
    }
}

fn default_codewhale_secrets_path() -> Result<PathBuf, SecretsError> {
    Ok(codewhale_paths::codewhale_home()
        .map_err(|error| {
            SecretsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, error))
        })?
        .ok_or_else(home_resolution_error)?
        .join("secrets")
        .join("secrets.json"))
}

fn legacy_deepseek_secrets_path() -> Result<PathBuf, SecretsError> {
    Ok(codewhale_paths::legacy_deepseek_home()
        .ok_or_else(home_resolution_error)?
        .join("secrets")
        .join("secrets.json"))
}

fn home_resolution_error() -> SecretsError {
    SecretsError::Io(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "could not resolve home directory for FileKeyringStore",
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretBackendSelection {
    File,
    System,
    Unknown,
}

/// Secret-store backend selected by configuration for a structural diagnostic.
///
/// This type deliberately describes only configuration and filesystem shape.
/// It never implies that a provider credential exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretBackendDiagnosticKind {
    /// The JSON file store is selected.
    File,
    /// The operating-system credential store is selected.
    System,
    /// The configured backend value is unsupported.
    Unknown,
}

/// Whether a secret-store path is present according to metadata only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretBackendPresence {
    /// A regular file exists at the resolved path.
    Present,
    /// No filesystem entry exists at the resolved path.
    Absent,
    /// Presence is unavailable or the entry is not a regular file.
    Unknown,
}

/// Scope of inspection performed for a structural secret-backend diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretBackendInspection {
    /// Only filesystem metadata was inspected; file contents were not opened.
    MetadataOnly,
    /// The backend was not constructed, probed, or read.
    NotProbed,
}

/// Secret-safe structural description of the configured credential backend.
///
/// File-backed diagnostics expose resolved paths and regular-file presence from
/// metadata without opening either store. System backends intentionally report
/// `unknown` / `not_probed`: constructing or probing an OS keyring can show a
/// user prompt even when no credential value is requested.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretBackendDiagnostic {
    /// Configured backend family.
    pub backend: SecretBackendDiagnosticKind,
    /// Inspection performed to produce this report.
    pub inspection: SecretBackendInspection,
    /// Canonical file-store path, when the file backend is selected.
    pub path: Option<PathBuf>,
    /// Metadata-only presence of the canonical file-store path.
    pub presence: SecretBackendPresence,
    /// Ambient legacy file-store path, suppressed by explicit `CODEWHALE_HOME`.
    pub legacy_path: Option<PathBuf>,
    /// Metadata-only presence of the legacy file-store path.
    pub legacy_presence: SecretBackendPresence,
}

/// Describe the configured credential backend without probing or reading it.
///
/// This function never constructs [`DefaultKeyringStore`], calls
/// [`KeyringStore::get`], opens a secret file, performs legacy migration, or
/// creates filesystem state. It is suitable for ordinary status and doctor
/// commands.
#[must_use]
pub fn diagnose_secret_backend() -> SecretBackendDiagnostic {
    match secret_backend_selection(configured_secret_backend().as_deref()) {
        SecretBackendSelection::File => {
            let (path, legacy_path) = FileKeyringStore::default_paths_read_only()
                .map(|(path, legacy)| (Some(path), legacy))
                .unwrap_or((None, None));
            SecretBackendDiagnostic {
                backend: SecretBackendDiagnosticKind::File,
                inspection: SecretBackendInspection::MetadataOnly,
                presence: metadata_presence(path.as_deref()),
                legacy_presence: metadata_presence(legacy_path.as_deref()),
                path,
                legacy_path,
            }
        }
        SecretBackendSelection::System => SecretBackendDiagnostic {
            backend: SecretBackendDiagnosticKind::System,
            inspection: SecretBackendInspection::NotProbed,
            path: None,
            presence: SecretBackendPresence::Unknown,
            legacy_path: None,
            legacy_presence: SecretBackendPresence::Unknown,
        },
        SecretBackendSelection::Unknown => SecretBackendDiagnostic {
            backend: SecretBackendDiagnosticKind::Unknown,
            inspection: SecretBackendInspection::NotProbed,
            path: None,
            presence: SecretBackendPresence::Unknown,
            legacy_path: None,
            legacy_presence: SecretBackendPresence::Unknown,
        },
    }
}

fn metadata_presence(path: Option<&Path>) -> SecretBackendPresence {
    let Some(path) = path else {
        return SecretBackendPresence::Unknown;
    };
    if let Some(parent) = path.parent() {
        for ancestor in parent.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            match fs::symlink_metadata(ancestor) {
                Ok(metadata)
                    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() =>
                {
                    return SecretBackendPresence::Unknown;
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return SecretBackendPresence::Absent;
                }
                Err(_) => return SecretBackendPresence::Unknown,
            }
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => SecretBackendPresence::Present,
        Ok(_) => SecretBackendPresence::Unknown,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => SecretBackendPresence::Absent,
        Err(_) => SecretBackendPresence::Unknown,
    }
}

fn secret_backend_selection(value: Option<&str>) -> SecretBackendSelection {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => SecretBackendSelection::File,
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "file" | "local" | "json" => SecretBackendSelection::File,
            "system" | "keyring" | "os" | "os-keyring" => SecretBackendSelection::System,
            _ => SecretBackendSelection::Unknown,
        },
    }
}

fn configured_secret_backend() -> Option<String> {
    std::env::var(SECRET_BACKEND_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var(LEGACY_SECRET_BACKEND_ENV).ok())
}

/// High-level facade combining a [`KeyringStore`] with environment variable fallbacks.
///
/// Lookup precedence: **secret store -> env -> none**. Callers that also
/// have a TOML config layer must wire that themselves at the very end
/// of the chain (the config crate handles this).
///
/// # Examples
///
/// ```no_run
/// use codewhale_secrets::Secrets;
///
/// let secrets = Secrets::auto_detect();
/// if let Some(key) = secrets.resolve("deepseek") {
///     // use the API key
/// }
/// ```
#[derive(Clone)]
pub struct Secrets {
    /// Underlying secret store backend.
    pub store: Arc<dyn KeyringStore>,
    /// Owner identifier within the secret store (typically `"deepseek"`).
    /// The `key` parameter passed to [`resolve`](Secrets::resolve) is
    /// forwarded to the store as-is, while environment variables are
    /// looked up by canonical provider name via [`env_for`].
    service: String,
}

/// Identifies which layer in the resolution chain supplied a secret.
///
/// Returned by [`Secrets::resolve_with_source`] so callers can
/// distinguish whether a value came from the configured store or from
/// a process environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSource {
    /// The secret was returned by the configured [`KeyringStore`] backend.
    Keyring,
    /// The secret was found in a process environment variable.
    Env,
}

impl std::fmt::Debug for Secrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Secrets")
            .field("backend", &self.store.backend_name())
            .field("service", &self.service)
            .finish()
    }
}

impl Secrets {
    /// Build a new facade around the given store, using the
    /// [`DEFAULT_SERVICE`] service name.
    #[must_use]
    pub fn new(store: Arc<dyn KeyringStore>) -> Self {
        Self {
            store,
            service: DEFAULT_SERVICE.to_string(),
        }
    }

    /// Auto-detect the best available backend based on the environment.
    ///
    /// Selection logic:
    /// 1. If [`SECRET_BACKEND_ENV`] is set to `system`/`keyring`/`os`/`os-keyring`,
    ///    probe the OS keyring. If the probe succeeds, use it; otherwise
    ///    fall back to the file-based store with a warning.
    /// 2. If the env var is unset, empty, or `file`/`local`/`json`, use
    ///    the file-based store directly.
    /// 3. If the env var is set to an unrecognised value, log a warning
    ///    and use the file-based store.
    pub fn auto_detect() -> Self {
        match secret_backend_selection(configured_secret_backend().as_deref()) {
            SecretBackendSelection::File => Self::file_backed_default(),
            SecretBackendSelection::Unknown => {
                tracing::warn!(
                    "{SECRET_BACKEND_ENV}/{LEGACY_SECRET_BACKEND_ENV} has an unsupported value; using file-backed secret store"
                );
                Self::file_backed_default()
            }
            SecretBackendSelection::System => {
                let default_store = DefaultKeyringStore::default();
                match default_store.probe() {
                    Ok(()) => Self::new(Arc::new(default_store)),
                    Err(err) => {
                        tracing::warn!(
                            "OS keyring unavailable ({err}); falling back to file-backed secret store"
                        );
                        Self::file_backed_default()
                    }
                }
            }
        }
    }

    /// Auto-detect a secret backend for diagnostics without permitting writes
    /// or legacy migration.
    ///
    /// The selected backend and lookup precedence match [`Self::auto_detect`],
    /// but file-backed lookup reads the Codewhale location first and the legacy
    /// location second instead of copying legacy entries into a new file. This
    /// lets status and doctor reports label a saved credential without changing
    /// user state.
    #[must_use]
    pub fn auto_detect_read_only() -> Self {
        match secret_backend_selection(configured_secret_backend().as_deref()) {
            SecretBackendSelection::File => Self::file_backed_read_only(),
            SecretBackendSelection::Unknown => {
                tracing::warn!(
                    "{SECRET_BACKEND_ENV}/{LEGACY_SECRET_BACKEND_ENV} has an unsupported value; using file-backed secret store"
                );
                Self::file_backed_read_only()
            }
            SecretBackendSelection::System => {
                let default_store = DefaultKeyringStore::default();
                match default_store.probe() {
                    Ok(()) => {
                        Self::new(Arc::new(ReadOnlyKeyringStore::new(Arc::new(default_store))))
                    }
                    Err(err) => {
                        tracing::warn!(
                            "OS keyring unavailable ({err}); falling back to file-backed secret store"
                        );
                        Self::file_backed_read_only()
                    }
                }
            }
        }
    }

    fn file_backed_default() -> Self {
        Self::file_backed_from_default_path(FileKeyringStore::default_path())
    }

    /// Build the writable default store only when the resolved path is safe.
    ///
    /// Keeping the resolution result as an argument gives the no-home and
    /// relative-path branches direct regression coverage. Both must refuse
    /// writes rather than placing credentials in the caller's workspace.
    fn file_backed_from_default_path(path_result: Result<PathBuf, SecretsError>) -> Self {
        // Never fall back to a workspace-relative secrets path. Writing
        // credential material beside the cwd is readable by tools and easy to
        // commit. If home resolution fails, use a write-refusing store.
        match path_result {
            Ok(path) if path.is_absolute() => Self::new(Arc::new(FileKeyringStore::new(path))),
            Ok(path) => {
                tracing::error!(
                    "refusing relative file-backed secret path {}; credentials will not be read or persisted",
                    path.display()
                );
                Self::read_only_empty_store()
            }
            Err(err) => {
                tracing::error!(
                    "could not resolve file-backed secret path ({err}); credentials will not be read or persisted"
                );
                Self::read_only_empty_store()
            }
        }
    }

    /// An unavailable default path must be hermetic: neither inspect an
    /// accidental workspace file nor create one.  The read-only wrapper keeps
    /// the public API's write failure explicit while reads safely report empty.
    fn read_only_empty_store() -> Self {
        Self::new(Arc::new(ReadOnlyKeyringStore::new(Arc::new(
            InMemoryKeyringStore::new(),
        ))))
    }

    /// Construct a file-backed diagnostic store without migration or write
    /// capability.
    ///
    /// This reads the Codewhale file first and the legacy file second (unless
    /// `CODEWHALE_HOME` is explicit), but never copies legacy entries into a
    /// primary store. It intentionally bypasses an opted-in OS keyring so
    /// callers that only need non-secret diagnostics do not cause a platform
    /// credential prompt.
    #[must_use]
    pub fn file_backed_read_only() -> Self {
        // Fail closed like the writable path in `file_backed_from_default_path`:
        // never fall back to a cwd-relative credential path. A planted
        // `.codewhale-secrets.json` beside the working directory must not
        // become the credential store when home resolution fails.
        match ReadOnlyFileKeyringStore::default_for_diagnostics() {
            Ok(store) => Self::new(Arc::new(store)),
            Err(err) => {
                tracing::error!(
                    "could not resolve the file-backed secret path ({err}); credentials will not be read. \
                     Fix: set CODEWHALE_HOME to an absolute path or make HOME/USERPROFILE resolvable"
                );
                Self::read_only_empty_store()
            }
        }
    }

    /// Construct the file-backed default backend directly.
    #[must_use]
    pub fn file_backed() -> Self {
        Self::file_backed_default()
    }

    /// Construct the opt-in OS credential backend, falling back to the
    /// file-backed store when the platform backend is unavailable.
    #[must_use]
    pub fn system_keyring() -> Self {
        let default_store = DefaultKeyringStore::default();
        match default_store.probe() {
            Ok(()) => Self::new(Arc::new(default_store)),
            Err(err) => {
                tracing::warn!(
                    "OS keyring unavailable ({err}); falling back to file-backed secret store"
                );
                Self::file_backed_default()
            }
        }
    }

    /// Backend label, suitable for `doctor` output.
    #[must_use]
    pub fn backend_name(&self) -> &'static str {
        self.store.backend_name()
    }

    /// Resolve a secret with `secret store → env → none` precedence.
    ///
    /// `name` is the canonical provider name or a supported provider alias.
    /// Empty strings on either layer are treated as "not set".
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<String> {
        self.resolve_with_source(name).map(|(value, _)| value)
    }

    /// Resolve a secret and report which layer supplied it.
    #[must_use]
    pub fn resolve_with_source(&self, name: &str) -> Option<(String, SecretSource)> {
        if let Ok(Some(v)) = self.store.get(name)
            && !v.trim().is_empty()
        {
            return Some((v, SecretSource::Keyring));
        }
        env_for(name).map(|value| (value, SecretSource::Env))
    }

    /// Convenience: write a secret through the underlying store.
    pub fn set(&self, name: &str, value: &str) -> Result<(), SecretsError> {
        self.store.set(name, value)
    }

    /// Convenience: delete a secret through the underlying store.
    pub fn delete(&self, name: &str) -> Result<(), SecretsError> {
        self.store.delete(name)
    }

    /// Convenience: read a secret directly (no env fallback).
    pub fn get(&self, name: &str) -> Result<Option<String>, SecretsError> {
        self.store.get(name)
    }

    /// Resolve a secret by key name with an optional source constraint.
    ///
    /// This is the fleet-worker secret resolution path. Unlike
    /// [`resolve`](Secrets::resolve), this does NOT map provider names
    /// to their canonical env vars — the caller controls the exact key
    /// and resolution order.
    ///
    /// `source_hint` controls the resolution order:
    /// - `Some("env")` — only check environment variables
    /// - `Some("keyring")` — only check the keyring/file store
    /// - `None` — try the store first, then fall back to environment
    #[must_use]
    pub fn resolve_direct(&self, key: &str, source_hint: Option<&str>) -> Option<String> {
        match source_hint {
            Some("env") => {
                // Only check process environment — skip the store entirely.
                std::env::var(key).ok().filter(|v| !v.trim().is_empty())
            }
            Some("keyring") | Some("file") => {
                // Only check the store backend.
                self.store
                    .get(key)
                    .ok()
                    .flatten()
                    .filter(|v| !v.trim().is_empty())
            }
            Some(_) | None => {
                // Default: store first, then env fallback.
                if let Ok(Some(v)) = self.store.get(key)
                    && !v.trim().is_empty()
                {
                    return Some(v);
                }
                std::env::var(key).ok().filter(|v| !v.trim().is_empty())
            }
        }
    }
}

/// Map a canonical provider name to its environment variable(s), returning
/// the first non-empty value found.
///
/// Provider names are case-insensitive. Supported providers and their
/// environment variables:
///
/// | Provider | Env var(s) |
/// |---|---|
/// | `deepseek` | `DEEPSEEK_API_KEY` |
/// | `openrouter` | `OPENROUTER_API_KEY` |
/// | `xiaomi-mimo` / `mimo` | `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` |
/// | `novita` / `novita-ai` | `NOVITA_API_KEY` |
/// | `nvidia` / `nvidia-nim` / `nim` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY` |
/// | `fireworks` / `fireworks-ai` | `FIREWORKS_API_KEY` |
/// | `together` / `togetherai` | `TOGETHER_API_KEY` |
/// | `deepinfra` | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` |
/// | `siliconflow` / `siliconflow-cn` | `SILICONFLOW_API_KEY` |
/// | `arcee` / `arcee-ai` | `ARCEE_API_KEY` |
/// | `moonshot` / `kimi` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` |
/// | `sglang` | `SGLANG_API_KEY` |
/// | `vllm` | `VLLM_API_KEY` |
/// | `ollama` | `OLLAMA_API_KEY` |
/// | `ollama-cloud` | `OLLAMA_CLOUD_API_KEY`, `OLLAMA_API_KEY` |
/// | `openai` | `OPENAI_API_KEY` |
/// | `atlascloud` / `atlas` | `ATLASCLOUD_API_KEY` |
/// | `volcengine` / `ark` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` |
/// | `wanjie` / `wanjie-ark` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` |
/// | `meta` / `muse-spark` | `META_MODEL_API_KEY`, `MODEL_API_KEY` |
/// | `xai` / `grok` | `XAI_API_KEY` |
/// | `telecomjs` / `tokenhub` | `TELECOMJS_API_KEY` |
/// | `edenai` / `eden-ai` | `EDENAI_API_KEY` |
/// | `concentrate` / `concentrate-ai` | `CONCENTRATE_API_KEY` |
///
/// Returns `None` if the provider is not recognised or none of its
/// candidate environment variables are set to a non-empty value.
#[must_use]
pub fn env_for(name: &str) -> Option<String> {
    let candidates: &[&str] = match name.to_ascii_lowercase().as_str() {
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => {
            &["XIAOMI_MIMO_API_KEY", "XIAOMI_API_KEY", "MIMO_API_KEY"]
        }
        "novita" | "novita-ai" | "novita_ai" => &["NOVITA_API_KEY"],
        "together" | "together-ai" | "together_ai" | "togetherai" => &["TOGETHER_API_KEY"],
        "deepinfra" | "deep-infra" | "deep_infra" => &["DEEPINFRA_API_KEY", "DEEPINFRA_TOKEN"],
        "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => &["NVIDIA_API_KEY", "NVIDIA_NIM_API_KEY"],
        "fireworks" | "fireworks-ai" => &["FIREWORKS_API_KEY"],
        "siliconflow" | "silicon-flow" | "silicon_flow" | "siliconflow-cn" | "siliconflow_cn"
        | "silicon-flow-cn" | "silicon_flow_cn" | "siliconflow-china" => &["SILICONFLOW_API_KEY"],
        "arcee" | "arcee-ai" | "arcee_ai" => &["ARCEE_API_KEY"],
        "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        "sglang" | "sg-lang" => &["SGLANG_API_KEY"],
        "vllm" | "v-llm" => &["VLLM_API_KEY"],
        "ollama" | "ollama-local" => &["OLLAMA_API_KEY"],
        "ollama-cloud" | "ollama_cloud" => &["OLLAMA_CLOUD_API_KEY", "OLLAMA_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
        "anthropic" | "claude" => &["ANTHROPIC_API_KEY"],
        "atlascloud" | "atlas-cloud" | "atlas_cloud" | "atlas" => &["ATLASCLOUD_API_KEY"],
        "volcengine" | "volcengine-ark" | "volcengine_ark" | "ark" | "volc-ark"
        | "volcengineark" => &[
            "VOLCENGINE_API_KEY",
            "VOLCENGINE_ARK_API_KEY",
            "ARK_API_KEY",
        ],
        "wanjie" | "wanjie-ark" | "wanjie_ark" | "ark-wanjie" | "ark_wanjie" | "wanjieark"
        | "wanjie-maas" | "wanjie_maas" | "wanjiemaas" => &[
            "WANJIE_ARK_API_KEY",
            "WANJIE_API_KEY",
            "WANJIE_MAAS_API_KEY",
        ],
        "sakana" | "sakana-ai" | "sakana_ai" | "fugu" => &["FUGU_API_KEY", "SAKANA_API_KEY"],
        "longcat" | "long-cat" | "meituan-longcat" | "meituan" => &["LONGCAT_API_KEY"],
        "opencode-go" | "opencode_go" | "opencodego" => &["OPENCODE_GO_API_KEY"],
        "opencode-zen" | "opencode_zen" | "opencodezen" | "zen" | "opencode" => {
            &["OPENCODE_ZEN_API_KEY", "OPENCODE_API_KEY"]
        }
        "meta" | "meta-ai" | "meta_ai" | "meta-model-api" | "meta_model_api" | "muse"
        | "muse-spark" => &["META_MODEL_API_KEY", "MODEL_API_KEY"],
        "xai" | "x-ai" | "x_ai" | "grok" => &["XAI_API_KEY"],
        "telecomjs" | "telecom-js" | "telecom_js" | "telecomjs-cn" | "tokenhub" => {
            &["TELECOMJS_API_KEY"]
        }
        "edenai" | "eden-ai" | "eden_ai" => &["EDENAI_API_KEY"],
        "concentrate" | "concentrate-ai" | "concentrate_ai" | "concentrateai" => {
            &["CONCENTRATE_API_KEY"]
        }
        "daytona" => &[DAYTONA_API_KEY_ENV, CWC_DAYTONA_TOKEN_ENV],
        // One Alibaba Cloud Model Studio account authenticates every plan /
        // dialect variant; all four names share one env convention.
        "modelstudio-token-plan"
        | "modelstudio_token_plan"
        | "modelstudio-token-plan-anthropic"
        | "modelstudio_token_plan_anthropic"
        | "modelstudio-coding-plan"
        | "modelstudio_coding_plan"
        | "modelstudio-coding-plan-anthropic"
        | "modelstudio_coding_plan_anthropic"
        | "modelstudio"
        | "dashscope"
        | "alibaba-token-plan"
        | "alibaba-coding-plan" => &["MODELSTUDIO_API_KEY", "DASHSCOPE_API_KEY"],
        _ => return None,
    };
    for var in candidates {
        if let Ok(value) = std::env::var(var)
            && !value.trim().is_empty()
        {
            return Some(value);
        }
    }
    None
}

/// Report whether a Daytona token is present without revealing it.
///
/// Order matches dispatch: secret-store slot `daytona`, then
/// [`DAYTONA_API_KEY_ENV`], then [`CWC_DAYTONA_TOKEN_ENV`].
#[must_use]
pub fn daytona_credential_source(secrets: &Secrets) -> Option<&'static str> {
    if secrets
        .get(DAYTONA_TOKEN_SLOT)
        .ok()
        .flatten()
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Some("secret-store");
    }
    for var in [DAYTONA_API_KEY_ENV, CWC_DAYTONA_TOKEN_ENV] {
        if std::env::var(var)
            .ok()
            .is_some_and(|value| !value.trim().is_empty())
        {
            return Some("env");
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serialise env-mutating tests: tests in this module poke
    /// `DEEPSEEK_API_KEY` etc., which is process-global.
    pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }

    fn clear_known_envs() {
        for var in [
            "CODEWHALE_HOME",
            "DEEPSEEK_API_KEY",
            "OPENROUTER_API_KEY",
            "NOVITA_API_KEY",
            "NVIDIA_API_KEY",
            "NVIDIA_NIM_API_KEY",
            "FIREWORKS_API_KEY",
            "TOGETHER_API_KEY",
            "DEEPINFRA_API_KEY",
            "DEEPINFRA_TOKEN",
            "SILICONFLOW_API_KEY",
            "ARCEE_API_KEY",
            "SGLANG_API_KEY",
            "VLLM_API_KEY",
            "OLLAMA_API_KEY",
            "OLLAMA_CLOUD_API_KEY",
            "OPENAI_API_KEY",
            "ATLASCLOUD_API_KEY",
            "WANJIE_ARK_API_KEY",
            "WANJIE_API_KEY",
            "WANJIE_MAAS_API_KEY",
            "XIAOMI_MIMO_API_KEY",
            "XIAOMI_API_KEY",
            "MIMO_API_KEY",
            "FUGU_API_KEY",
            "SAKANA_API_KEY",
            "LONGCAT_API_KEY",
            "OPENCODE_GO_API_KEY",
            "OPENCODE_ZEN_API_KEY",
            "OPENCODE_API_KEY",
            "META_MODEL_API_KEY",
            "MODEL_API_KEY",
            "XAI_API_KEY",
            "TELECOMJS_API_KEY",
            "EDENAI_API_KEY",
            "CONCENTRATE_API_KEY",
            "MODELSTUDIO_API_KEY",
            "DASHSCOPE_API_KEY",
            DAYTONA_API_KEY_ENV,
            CWC_DAYTONA_TOKEN_ENV,
            SECRET_BACKEND_ENV,
            LEGACY_SECRET_BACKEND_ENV,
        ] {
            // Safety: tests serialise on env_lock(); the broader
            // workspace has the same pattern in `crates/config`.
            unsafe { std::env::remove_var(var) };
        }
    }

    struct EnvVarGuard {
        name: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(name);
            unsafe { std::env::set_var(name, value) };
            Self { name, previous }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => unsafe { std::env::set_var(self.name, value) },
                None => unsafe { std::env::remove_var(self.name) },
            }
        }
    }

    /// Live check for #5172: on macOS/Windows the probe used to return Ok
    /// without touching the backend at all. Run explicitly with
    /// `cargo test -p codewhale-secrets -- --ignored` on a desktop machine:
    /// a healthy native keyring answers a read of the deliberately absent
    /// `__probe__` entry with NoEntry, silently, and the probe succeeds.
    #[test]
    #[ignore = "touches the real OS keyring; run on a desktop machine"]
    fn probe_performs_a_real_backend_read() {
        let store = DefaultKeyringStore::new("codewhale-probe-live-check");
        store
            .probe()
            .expect("the native keyring backend should be reachable on this machine");
    }

    #[test]
    fn backend_selection_defaults_to_file() {
        assert_eq!(secret_backend_selection(None), SecretBackendSelection::File);
        assert_eq!(
            secret_backend_selection(Some("")),
            SecretBackendSelection::File
        );
        assert_eq!(
            secret_backend_selection(Some("  file  ")),
            SecretBackendSelection::File
        );
    }

    #[test]
    fn backend_selection_accepts_explicit_system_keyring() {
        assert_eq!(
            secret_backend_selection(Some("system")),
            SecretBackendSelection::System
        );
        assert_eq!(
            secret_backend_selection(Some("keyring")),
            SecretBackendSelection::System
        );
        assert_eq!(
            secret_backend_selection(Some("os-keyring")),
            SecretBackendSelection::System
        );
    }

    #[test]
    fn auto_detect_is_file_backed_by_default() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());

        let secrets = Secrets::auto_detect();

        assert_eq!(secrets.backend_name(), FILE_BACKEND_LABEL);
    }

    #[test]
    fn auto_detect_honors_explicit_file_backend() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var(SECRET_BACKEND_ENV, "local") };

        let secrets = Secrets::auto_detect();

        assert_eq!(secrets.backend_name(), FILE_BACKEND_LABEL);
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var(SECRET_BACKEND_ENV) };
    }

    #[test]
    fn read_only_auto_detect_reads_legacy_without_migrating_or_allowing_writes() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");
        let legacy = tmp
            .path()
            .join(".deepseek")
            .join("secrets")
            .join("secrets.json");
        let primary = tmp
            .path()
            .join(".codewhale")
            .join("secrets")
            .join("secrets.json");
        FileKeyringStore::new(&legacy)
            .set("moonshot", "fixture-legacy-value")
            .unwrap();

        let secrets = Secrets::auto_detect_read_only();

        assert_eq!(
            secrets.get("moonshot").unwrap().as_deref(),
            Some("fixture-legacy-value")
        );
        assert!(
            !primary.exists(),
            "diagnostic lookup must not migrate the legacy store"
        );
        assert!(
            matches!(
                secrets.set("moonshot", "replacement"),
                Err(SecretsError::ReadOnly)
            ),
            "the diagnostic secret facade must refuse writes"
        );
        assert!(
            !primary.exists(),
            "a refused diagnostic write must not create the primary store"
        );
    }

    #[test]
    fn read_only_auto_detect_respects_explicit_codewhale_home_isolation() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let codewhale_home = tmp.path().join("isolated-codewhale-home");
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");
        let legacy = tmp
            .path()
            .join(".deepseek")
            .join("secrets")
            .join("secrets.json");
        let primary = codewhale_home.join("secrets").join("secrets.json");
        FileKeyringStore::new(&legacy)
            .set("deepseek", "synthetic-ambient-legacy-value")
            .unwrap();

        let secrets = Secrets::auto_detect_read_only();

        assert_eq!(
            secrets.get("deepseek").unwrap(),
            None,
            "an explicit CODEWHALE_HOME must not read ambient legacy secrets"
        );
        assert!(
            !primary.exists(),
            "diagnostic lookup must not create an isolated primary store"
        );
        assert!(
            matches!(
                secrets.set("deepseek", "replacement"),
                Err(SecretsError::ReadOnly)
            ),
            "the isolated diagnostic facade must refuse writes"
        );
        assert!(
            !primary.exists(),
            "a refused isolated diagnostic write must not create the primary store"
        );
    }

    /// Cwd is process-global, so tests that move it serialise on `env_lock`
    /// like the env-mutating tests and restore on drop.
    struct CwdGuard {
        previous: PathBuf,
    }

    impl CwdGuard {
        fn enter(path: &Path) -> Self {
            let previous = std::env::current_dir().unwrap();
            std::env::set_current_dir(path).unwrap();
            Self { previous }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.previous).unwrap();
        }
    }

    #[test]
    fn file_backed_read_only_never_reads_a_cwd_relative_store() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        // A relative override fails home resolution deterministically, which
        // used to fall back to a planted `.codewhale-secrets.json` in the cwd.
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", "relative-codewhale-home");
        let planted = tmp.path().join(".codewhale-secrets.json");
        std::fs::write(
            &planted,
            r#"{"entries":{"deepseek":"planted-cwd-credential"}}"#,
        )
        .unwrap();
        let _cwd = CwdGuard::enter(tmp.path());

        let secrets = Secrets::file_backed_read_only();

        assert_eq!(
            secrets.get("deepseek").unwrap(),
            None,
            "a failed home resolution must not turn a planted cwd file into the credential store"
        );
        assert!(
            matches!(
                secrets.set("deepseek", "replacement"),
                Err(SecretsError::ReadOnly)
            ),
            "the failed-resolution diagnostic facade must still refuse writes"
        );
    }

    #[test]
    fn read_only_auto_detect_reads_the_explicit_primary_store() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let codewhale_home = tmp.path().join("isolated-codewhale-home");
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let _backend = EnvVarGuard::set(SECRET_BACKEND_ENV, "file");
        let primary = codewhale_home.join("secrets").join("secrets.json");
        FileKeyringStore::new(&primary)
            .set("deepseek", "synthetic-isolated-primary-value")
            .unwrap();

        let secrets = Secrets::auto_detect_read_only();

        assert_eq!(
            secrets.get("deepseek").unwrap().as_deref(),
            Some("synthetic-isolated-primary-value")
        );
    }

    #[test]
    fn auto_detect_honors_legacy_backend_env_alias() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        unsafe { std::env::set_var(LEGACY_SECRET_BACKEND_ENV, "local") };

        let secrets = Secrets::auto_detect();

        assert_eq!(secrets.backend_name(), FILE_BACKEND_LABEL);
        clear_known_envs();
    }

    #[test]
    fn file_default_path_uses_codewhale_home() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());

        let path = FileKeyringStore::default_path().unwrap();

        assert_eq!(
            path,
            tmp.path()
                .join(".codewhale")
                .join("secrets")
                .join("secrets.json")
        );
    }

    #[test]
    fn file_default_path_honors_codewhale_home() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let custom = tmp.path().join("custom-codewhale");
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &custom);

        let path = FileKeyringStore::default_path().unwrap();

        assert_eq!(path, custom.join("secrets").join("secrets.json"));
    }

    #[test]
    fn file_default_path_migrates_legacy_entries_to_codewhale() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let legacy = tmp
            .path()
            .join(".deepseek")
            .join("secrets")
            .join("secrets.json");
        FileKeyringStore::new(legacy.clone())
            .set("xiaomi-mimo", "legacy-mimo")
            .unwrap();

        let primary = FileKeyringStore::default_path().unwrap();
        let primary_store = FileKeyringStore::new(primary.clone());

        assert_eq!(
            primary,
            tmp.path()
                .join(".codewhale")
                .join("secrets")
                .join("secrets.json")
        );
        assert_eq!(
            primary_store.get("xiaomi-mimo").unwrap().as_deref(),
            Some("legacy-mimo")
        );
        assert!(
            legacy.exists(),
            "migration copies; it does not delete legacy data"
        );
    }

    #[test]
    fn file_default_path_migration_preserves_primary_values() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let legacy = tmp
            .path()
            .join(".deepseek")
            .join("secrets")
            .join("secrets.json");
        let primary = tmp
            .path()
            .join(".codewhale")
            .join("secrets")
            .join("secrets.json");
        FileKeyringStore::new(legacy)
            .set("openrouter", "legacy-openrouter")
            .unwrap();
        let primary_store = FileKeyringStore::new(primary.clone());
        primary_store
            .set("openrouter", "primary-openrouter")
            .unwrap();

        let resolved = FileKeyringStore::default_path().unwrap();

        assert_eq!(resolved, primary);
        assert_eq!(
            primary_store.get("openrouter").unwrap().as_deref(),
            Some("primary-openrouter")
        );
    }

    #[test]
    fn in_memory_store_round_trips() {
        let store = InMemoryKeyringStore::new();
        assert_eq!(store.get("deepseek").unwrap(), None);
        store.set("deepseek", "sk-test").unwrap();
        assert_eq!(store.get("deepseek").unwrap(), Some("sk-test".to_string()));
        store.set("deepseek", "sk-replaced").unwrap();
        assert_eq!(
            store.get("deepseek").unwrap(),
            Some("sk-replaced".to_string())
        );
        store.delete("deepseek").unwrap();
        assert_eq!(store.get("deepseek").unwrap(), None);
        // Deleting an absent key is a no-op.
        store.delete("missing").unwrap();
    }

    #[test]
    fn resolve_prefers_keyring_over_env() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-key") };

        let store = Arc::new(InMemoryKeyringStore::new());
        store.set("deepseek", "ring-key").unwrap();
        let secrets = Secrets::new(store);

        assert_eq!(secrets.resolve("deepseek").as_deref(), Some("ring-key"));
        assert_eq!(
            secrets.resolve_with_source("deepseek"),
            Some(("ring-key".to_string(), SecretSource::Keyring))
        );
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn resolve_falls_back_to_env_when_keyring_empty() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-fallback") };

        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));
        assert_eq!(secrets.resolve("deepseek").as_deref(), Some("env-fallback"));
        assert_eq!(
            secrets.resolve_with_source("deepseek"),
            Some(("env-fallback".to_string(), SecretSource::Env))
        );
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn resolve_returns_none_when_both_layers_empty() {
        let _lock = env_lock();
        clear_known_envs();
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));
        assert_eq!(secrets.resolve("deepseek"), None);
    }

    #[test]
    fn resolve_treats_blank_keyring_value_as_unset() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "env-real") };

        let store = Arc::new(InMemoryKeyringStore::new());
        store.set("deepseek", "   ").unwrap();
        let secrets = Secrets::new(store);
        assert_eq!(secrets.resolve("deepseek").as_deref(), Some("env-real"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPSEEK_API_KEY") };
    }

    #[test]
    fn nvidia_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe {
            std::env::set_var("NVIDIA_API_KEY", "nvidia-key");
            std::env::set_var("NVIDIA_NIM_API_KEY", "nim-key");
        }
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));
        for alias in ["nvidia", "nvidia-nim", "nvidia_nim", "nim"] {
            assert_eq!(
                secrets.resolve(alias).as_deref(),
                Some("nvidia-key"),
                "NVIDIA_API_KEY should take precedence for {alias}"
            );
        }

        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("NVIDIA_API_KEY") };
        for alias in ["nvidia", "nvidia-nim", "nvidia_nim", "nim"] {
            assert_eq!(
                secrets.resolve(alias).as_deref(),
                Some("nim-key"),
                "NVIDIA_NIM_API_KEY should resolve for {alias}"
            );
        }
        clear_known_envs();
    }

    #[test]
    fn nvidia_env_aliases_do_not_consume_deepseek_credentials() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPSEEK_API_KEY", "deepseek-key") };
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));

        for alias in ["nvidia", "nvidia-nim", "nvidia_nim", "nim"] {
            assert_eq!(
                secrets.resolve(alias),
                None,
                "DeepSeek credentials must stay isolated from {alias}"
            );
        }
        assert_eq!(secrets.resolve("deepseek").as_deref(), Some("deepseek-key"));
        clear_known_envs();
    }

    #[test]
    fn atlascloud_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("ATLASCLOUD_API_KEY", "atlas-key") };

        assert_eq!(env_for("atlascloud").as_deref(), Some("atlas-key"));
        assert_eq!(env_for("atlas").as_deref(), Some("atlas-key"));
        assert_eq!(env_for("atlas-cloud").as_deref(), Some("atlas-key"));

        clear_known_envs();
    }

    #[test]
    fn sakana_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("FUGU_API_KEY", "fugu-key") };

        assert_eq!(env_for("sakana").as_deref(), Some("fugu-key"));
        assert_eq!(env_for("sakana-ai").as_deref(), Some("fugu-key"));
        assert_eq!(env_for("sakana_ai").as_deref(), Some("fugu-key"));
        assert_eq!(env_for("fugu").as_deref(), Some("fugu-key"));

        clear_known_envs();
        unsafe { std::env::set_var("SAKANA_API_KEY", "sakana-key") };
        assert_eq!(env_for("sakana").as_deref(), Some("sakana-key"));

        clear_known_envs();
    }

    #[test]
    fn wanjie_ark_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("WANJIE_API_KEY", "wanjie-key") };

        assert_eq!(env_for("wanjie-ark").as_deref(), Some("wanjie-key"));
        assert_eq!(env_for("ark_wanjie").as_deref(), Some("wanjie-key"));
        assert_eq!(env_for("wanjie-maas").as_deref(), Some("wanjie-key"));

        clear_known_envs();
    }

    #[test]
    fn xai_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("XAI_API_KEY", "xai-key") };

        assert_eq!(env_for("xai").as_deref(), Some("xai-key"));
        assert_eq!(env_for("x-ai").as_deref(), Some("xai-key"));
        assert_eq!(env_for("x_ai").as_deref(), Some("xai-key"));
        assert_eq!(env_for("grok").as_deref(), Some("xai-key"));

        clear_known_envs();
    }

    #[test]
    fn telecomjs_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("TELECOMJS_API_KEY", "telecom-key") };

        for alias in [
            "telecomjs",
            "telecom-js",
            "telecom_js",
            "telecomjs-cn",
            "tokenhub",
        ] {
            assert_eq!(env_for(alias).as_deref(), Some("telecom-key"), "{alias}");
        }

        clear_known_envs();
    }

    #[test]
    fn edenai_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("EDENAI_API_KEY", "eden-key") };

        for alias in ["edenai", "eden-ai", "eden_ai"] {
            assert_eq!(env_for(alias).as_deref(), Some("eden-key"), "{alias}");
        }

        clear_known_envs();
    }

    #[test]
    fn concentrate_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("CONCENTRATE_API_KEY", "concentrate-key") };

        for alias in [
            "concentrate",
            "concentrate-ai",
            "concentrate_ai",
            "concentrateai",
        ] {
            assert_eq!(
                env_for(alias).as_deref(),
                Some("concentrate-key"),
                "{alias}"
            );
        }
        // The gateway key is its own slot: no other provider's env name feeds it
        // and it feeds no other provider.
        assert_eq!(env_for("edenai"), None);
        assert_eq!(env_for("openrouter"), None);

        clear_known_envs();
    }

    #[test]
    fn opencode_go_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("OPENCODE_GO_API_KEY", "go-key") };

        for alias in ["opencode-go", "opencode_go", "opencodego"] {
            assert_eq!(env_for(alias).as_deref(), Some("go-key"), "{alias}");
        }

        clear_known_envs();
    }

    #[test]
    fn modelstudio_variants_share_one_env_convention() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("MODELSTUDIO_API_KEY", "ms-key") };

        for alias in [
            "modelstudio-token-plan",
            "modelstudio-token-plan-anthropic",
            "modelstudio-coding-plan",
            "modelstudio-coding-plan-anthropic",
            "modelstudio",
            "dashscope",
            "alibaba-token-plan",
            "alibaba-coding-plan",
        ] {
            assert_eq!(env_for(alias).as_deref(), Some("ms-key"), "{alias}");
        }

        clear_known_envs();
        unsafe { std::env::set_var("DASHSCOPE_API_KEY", "dashscope-key") };
        assert_eq!(
            env_for("modelstudio-token-plan").as_deref(),
            Some("dashscope-key"),
            "DASHSCOPE_API_KEY is the fallback for the same account"
        );

        clear_known_envs();
    }

    #[test]
    fn opencode_zen_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("OPENCODE_ZEN_API_KEY", "zen-key") };

        for alias in [
            "opencode-zen",
            "opencode_zen",
            "opencodezen",
            "zen",
            "opencode",
        ] {
            assert_eq!(env_for(alias).as_deref(), Some("zen-key"), "{alias}");
        }

        clear_known_envs();
    }

    #[test]
    fn meta_model_api_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("MODEL_API_KEY", "meta-key") };

        for alias in [
            "meta",
            "meta-ai",
            "meta_ai",
            "meta-model-api",
            "meta_model_api",
            "muse",
            "muse-spark",
        ] {
            assert_eq!(env_for(alias).as_deref(), Some("meta-key"), "{alias}");
        }

        clear_known_envs();
        unsafe { std::env::set_var("META_MODEL_API_KEY", "meta-prefixed-key") };
        assert_eq!(env_for("meta").as_deref(), Some("meta-prefixed-key"),);

        clear_known_envs();
    }

    #[test]
    fn xiaomi_mimo_env_aliases_resolve() {
        let _guard = env_lock();
        clear_known_envs();
        unsafe { std::env::set_var("MIMO_API_KEY", "mimo-key") };

        assert_eq!(env_for("xiaomi-mimo").as_deref(), Some("mimo-key"));
        assert_eq!(env_for("xiaomimimo").as_deref(), Some("mimo-key"));
        assert_eq!(env_for("mimo").as_deref(), Some("mimo-key"));
        assert_eq!(env_for("xiaomi").as_deref(), Some("mimo-key"));

        clear_known_envs();

        unsafe { std::env::set_var("XIAOMI_API_KEY", "xiaomi-key") };
        assert_eq!(env_for("xiaomi-mimo").as_deref(), Some("xiaomi-key"));
        clear_known_envs();
    }

    #[test]
    fn fireworks_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("FIREWORKS_API_KEY", "fw-key") };

        assert_eq!(env_for("fireworks").as_deref(), Some("fw-key"));
        assert_eq!(env_for("fireworks-ai").as_deref(), Some("fw-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("FIREWORKS_API_KEY") };
    }

    #[test]
    fn together_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("TOGETHER_API_KEY", "together-key") };

        // Canonical id plus the legacy hyphen/underscore spellings AND the
        // separator-free `togetherai` id Models.dev publishes must all resolve.
        assert_eq!(env_for("together").as_deref(), Some("together-key"));
        assert_eq!(env_for("together-ai").as_deref(), Some("together-key"));
        assert_eq!(env_for("together_ai").as_deref(), Some("together-key"));
        assert_eq!(env_for("togetherai").as_deref(), Some("together-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("TOGETHER_API_KEY") };
    }

    #[test]
    fn deepinfra_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPINFRA_API_KEY", "di-key") };

        assert_eq!(env_for("deepinfra").as_deref(), Some("di-key"));
        assert_eq!(env_for("deep-infra").as_deref(), Some("di-key"));
        assert_eq!(env_for("deep_infra").as_deref(), Some("di-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPINFRA_API_KEY") };

        // The DEEPINFRA_TOKEN fallback is honored when the primary key is unset.
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("DEEPINFRA_TOKEN", "di-token") };
        assert_eq!(env_for("deepinfra").as_deref(), Some("di-token"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("DEEPINFRA_TOKEN") };
    }

    #[test]
    fn novita_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("NOVITA_API_KEY", "novita-key") };

        assert_eq!(env_for("novita").as_deref(), Some("novita-key"));
        // `novita-ai` is the Models.dev provider id (Refs #4186).
        assert_eq!(env_for("novita-ai").as_deref(), Some("novita-key"));
        assert_eq!(env_for("novita_ai").as_deref(), Some("novita-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("NOVITA_API_KEY") };
    }

    #[test]
    fn siliconflow_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("SILICONFLOW_API_KEY", "sf-key") };

        assert_eq!(env_for("siliconflow").as_deref(), Some("sf-key"));
        assert_eq!(env_for("silicon-flow").as_deref(), Some("sf-key"));
        assert_eq!(env_for("silicon_flow").as_deref(), Some("sf-key"));
        assert_eq!(env_for("siliconflow-cn").as_deref(), Some("sf-key"));
        assert_eq!(env_for("silicon_flow_cn").as_deref(), Some("sf-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("SILICONFLOW_API_KEY") };
    }

    #[test]
    fn arcee_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("ARCEE_API_KEY", "arcee-key") };

        assert_eq!(env_for("arcee").as_deref(), Some("arcee-key"));
        assert_eq!(env_for("arcee-ai").as_deref(), Some("arcee-key"));
        assert_eq!(env_for("arcee_ai").as_deref(), Some("arcee-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("ARCEE_API_KEY") };
    }

    #[test]
    fn moonshot_kimi_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("KIMI_API_KEY", "kimi-key") };

        assert_eq!(env_for("moonshot").as_deref(), Some("kimi-key"));
        assert_eq!(env_for("moonshot-ai").as_deref(), Some("kimi-key"));
        assert_eq!(env_for("kimi").as_deref(), Some("kimi-key"));
        assert_eq!(env_for("kimi-k2").as_deref(), Some("kimi-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("KIMI_API_KEY") };
    }

    #[test]
    fn sglang_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("SGLANG_API_KEY", "sglang-key") };

        assert_eq!(env_for("sglang").as_deref(), Some("sglang-key"));
        assert_eq!(env_for("sg-lang").as_deref(), Some("sglang-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("SGLANG_API_KEY") };
    }

    #[test]
    fn vllm_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("VLLM_API_KEY", "vllm-key") };

        assert_eq!(env_for("vllm").as_deref(), Some("vllm-key"));
        assert_eq!(env_for("v-llm").as_deref(), Some("vllm-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("VLLM_API_KEY") };
    }

    #[test]
    fn ollama_env_aliases_resolve() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::set_var("OLLAMA_API_KEY", "ollama-key") };

        assert_eq!(env_for("ollama").as_deref(), Some("ollama-key"));
        assert_eq!(env_for("ollama-local").as_deref(), Some("ollama-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("OLLAMA_API_KEY") };
    }

    #[test]
    fn ollama_cloud_env_prefers_pi_name_then_official_name() {
        let _lock = env_lock();
        clear_known_envs();
        // Safety: env mutation guarded by env_lock().
        unsafe {
            std::env::set_var("OLLAMA_CLOUD_API_KEY", "cloud-specific-key");
            std::env::set_var("OLLAMA_API_KEY", "official-fallback-key");
        }

        assert_eq!(
            env_for("ollama-cloud").as_deref(),
            Some("cloud-specific-key")
        );
        assert_eq!(
            env_for("ollama_cloud").as_deref(),
            Some("cloud-specific-key")
        );
        // The local identity stays on its original, keyless-provider env
        // contract and never consumes the cloud-specific compatibility name.
        assert_eq!(env_for("ollama").as_deref(), Some("official-fallback-key"));

        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("OLLAMA_CLOUD_API_KEY") };
        assert_eq!(
            env_for("ollama-cloud").as_deref(),
            Some("official-fallback-key")
        );
        clear_known_envs();
    }

    #[cfg(unix)]
    #[test]
    fn file_store_round_trips_with_secure_perms() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("secrets.json");
        let store = FileKeyringStore::new(path.clone());
        assert_eq!(store.get("deepseek").unwrap(), None);
        store.set("deepseek", "sk-disk").unwrap();
        assert_eq!(store.get("deepseek").unwrap(), Some("sk-disk".to_string()));

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");

        store.set("openrouter", "or-disk").unwrap();
        assert_eq!(
            store.get("openrouter").unwrap(),
            Some("or-disk".to_string())
        );
        // First entry must still be intact.
        assert_eq!(store.get("deepseek").unwrap(), Some("sk-disk".to_string()));

        store.delete("deepseek").unwrap();
        assert_eq!(store.get("deepseek").unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn file_store_rejects_world_readable_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        fs::write(&path, "{\"entries\":{\"deepseek\":\"leak\"}}").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        let store = FileKeyringStore::new(path);
        let err = store.get("deepseek").unwrap_err();
        assert!(
            matches!(err, SecretsError::InsecurePermissions { .. }),
            "unexpected error: {err}"
        );
    }

    // Regression for #281: `set` and `delete` used to call
    // `load_unlocked().unwrap_or_default()`, which silently wiped every
    // existing secret whenever the read failed (insecure permissions,
    // corrupt JSON, or any other I/O error).

    #[cfg(unix)]
    #[test]
    fn file_store_set_does_not_clobber_secrets_when_perms_are_bad() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let original = "{\"entries\":{\"deepseek\":\"sk-keep\",\"nvidia\":\"nv-keep\"}}";
        fs::write(&path, original).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        let store = FileKeyringStore::new(path.clone());
        let err = store.set("openrouter", "or-new").unwrap_err();
        assert!(
            matches!(err, SecretsError::InsecurePermissions { .. }),
            "set must surface the read error rather than overwriting; got: {err}"
        );

        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(
            on_disk, original,
            "set must not modify the file when load_unlocked errored"
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_store_delete_does_not_clobber_secrets_when_perms_are_bad() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        let original = "{\"entries\":{\"deepseek\":\"sk-keep\",\"nvidia\":\"nv-keep\"}}";
        fs::write(&path, original).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&path, perms).unwrap();

        let store = FileKeyringStore::new(path.clone());
        let err = store.delete("nvidia").unwrap_err();
        assert!(
            matches!(err, SecretsError::InsecurePermissions { .. }),
            "delete must surface the read error rather than wiping the file; got: {err}"
        );
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, original);
    }

    #[test]
    fn file_store_set_does_not_clobber_secrets_when_json_is_corrupt() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secrets.json");
        // Corrupt JSON. Permissions ok where unix; on Windows the perm-check
        // doesn't run so we exercise the json-error path directly.
        fs::write(&path, "{ this is not valid json").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms).unwrap();
        }

        let store = FileKeyringStore::new(path.clone());
        let err = store.set("deepseek", "sk-new").unwrap_err();
        assert!(
            matches!(err, SecretsError::Json(_)),
            "set must surface the parse error rather than wiping the file; got: {err}"
        );
        let on_disk = fs::read_to_string(&path).unwrap();
        assert_eq!(on_disk, "{ this is not valid json");
    }

    #[test]
    fn file_store_set_still_creates_file_when_missing() {
        // Regression guard: the #281 fix removed `unwrap_or_default()` from
        // the load call. Make sure the original first-write-creates-the-file
        // ergonomic still works — `load_unlocked` returns `Ok(default)` for
        // a missing file, so the `?` should pass through cleanly.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("secrets.json");
        let store = FileKeyringStore::new(path.clone());

        store.set("deepseek", "sk-fresh").unwrap();
        assert_eq!(store.get("deepseek").unwrap(), Some("sk-fresh".to_string()));
    }

    #[test]
    fn file_store_default_path_uses_home() {
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());

        let path = FileKeyringStore::default_path().unwrap();
        assert_eq!(
            path,
            tmp.path()
                .join(".codewhale")
                .join("secrets")
                .join("secrets.json")
        );
    }

    #[test]
    fn default_path_with_explicit_codewhale_home_does_not_migrate_ambient_legacy() {
        // FR003-C001: explicit CODEWHALE_HOME must not silently import ambient
        // `$HOME/.deepseek/secrets` credentials into the isolated home.
        let _lock = env_lock();
        clear_known_envs();
        let tmp = tempfile::tempdir().unwrap();
        let codewhale_home = tmp.path().join("isolated-codewhale-home");
        let _home = EnvVarGuard::set("HOME", tmp.path());
        let _userprofile = EnvVarGuard::set("USERPROFILE", tmp.path());
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        let legacy = tmp
            .path()
            .join(".deepseek")
            .join("secrets")
            .join("secrets.json");
        FileKeyringStore::new(&legacy)
            .set("deepseek", "synthetic-ambient-legacy-value")
            .unwrap();

        let path = FileKeyringStore::default_path().unwrap();
        assert_eq!(path, codewhale_home.join("secrets").join("secrets.json"));
        assert!(
            !path.exists(),
            "explicit CODEWHALE_HOME must not create/migrate a primary store from ambient legacy"
        );

        let secrets = Secrets::auto_detect();
        assert_eq!(
            secrets.get("deepseek").unwrap(),
            None,
            "explicit CODEWHALE_HOME must not surface ambient legacy credentials"
        );
    }

    #[test]
    fn file_backed_default_refuses_relative_secret_path() {
        // FR003-C002: a relative fallback would resolve against the workspace
        // and risk committing credentials. It must be write-refusing instead.
        let secrets =
            Secrets::file_backed_from_default_path(Ok(PathBuf::from(".codewhale-secrets.json")));
        assert!(matches!(
            secrets.set("deepseek", "must-not-land-relative"),
            Err(SecretsError::ReadOnly)
        ));
        assert_eq!(
            secrets.get("deepseek").unwrap(),
            None,
            "unsafe relative fallback must not read a workspace secret file"
        );
    }

    #[test]
    fn file_backed_default_refuses_writes_when_home_resolution_fails() {
        // Force the exact fallback branch instead of relying on the shared
        // platform-home resolver, which normally succeeds with HOME unset.
        let err = SecretsError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "synthetic unresolved home",
        ));
        let secrets = Secrets::file_backed_from_default_path(Err(err));
        assert!(matches!(
            secrets.set("deepseek", "must-not-persist"),
            Err(SecretsError::ReadOnly)
        ));
        assert_eq!(secrets.get("deepseek").unwrap(), None);
    }

    #[test]
    fn daytona_slot_resolves_secret_store_then_dispatch_envs() {
        let _lock = env_lock();
        clear_known_envs();
        let secrets = Secrets::new(std::sync::Arc::new(InMemoryKeyringStore::new()));
        assert_eq!(daytona_credential_source(&secrets), None);
        assert_eq!(DAYTONA_TOKEN_SLOT, "daytona");

        secrets.set(DAYTONA_TOKEN_SLOT, "dtn_store").unwrap();
        assert_eq!(daytona_credential_source(&secrets), Some("secret-store"));
        secrets.delete(DAYTONA_TOKEN_SLOT).unwrap();

        let _key = EnvVarGuard::set(DAYTONA_API_KEY_ENV, "dtn_env");
        assert_eq!(daytona_credential_source(&secrets), Some("env"));
        assert_eq!(env_for("daytona").as_deref(), Some("dtn_env"));
    }

    #[path = "diagnostic_tests.rs"]
    mod diagnostic_tests;
}
