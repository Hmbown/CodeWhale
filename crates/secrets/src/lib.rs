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

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Default OS keychain service name. Kept as `deepseek` for compatibility
/// with credentials saved before the CodeWhale rename. macOS users can verify
/// entries with `security find-generic-password -s deepseek -a <provider>`.
pub const DEFAULT_SERVICE: &str = "deepseek";
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
/// - **Linux**: Secret Service (GNOME Keyring / kwallet via dbus)
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
    pub fn probe(&self) -> Result<(), SecretsError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            // `Entry::new` is enough to validate the native macOS/Windows
            // backend path. Avoid a dummy read there because it can trigger
            // a second user-visible Keychain/Credential Manager access before
            // the real provider key lookup.
            let entry = keyring::Entry::new(&self.service, "__probe__")
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            {
                let _ = entry;
                Ok(())
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = &self.service;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }
}

impl KeyringStore for DefaultKeyringStore {
    fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(err) => Err(SecretsError::Keyring(err.to_string())),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = key;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), SecretsError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            entry
                .set_password(value)
                .map_err(|err| SecretsError::Keyring(err.to_string()))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (key, value);
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn delete(&self, key: &str) -> Result<(), SecretsError> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let entry = keyring::Entry::new(&self.service, key)
                .map_err(|err| SecretsError::Keyring(err.to_string()))?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(err) => Err(SecretsError::Keyring(err.to_string())),
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = key;
            Err(SecretsError::Keyring(unsupported_keyring_message()))
        }
    }

    fn backend_name(&self) -> &'static str {
        "system keyring"
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
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
    /// copied into the CodeWhale store.
    pub fn default_path() -> Result<PathBuf, SecretsError> {
        let primary = default_codewhale_secrets_path()?;
        let legacy = legacy_deepseek_secrets_path()?;
        if let Err(err) = Self::migrate_legacy_file_if_needed(&primary, &legacy) {
            tracing::warn!(
                "could not migrate legacy secret store from {} to {}: {err}",
                legacy.display(),
                primary.display()
            );
        }
        Ok(primary)
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

    fn home_dir() -> Result<PathBuf, SecretsError> {
        for var in ["HOME", "USERPROFILE"] {
            if let Ok(value) = std::env::var(var) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    return Ok(PathBuf::from(trimmed));
                }
            }
        }

        dirs::home_dir().ok_or_else(|| {
            SecretsError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "could not resolve home directory for FileKeyringStore",
            ))
        })
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
        fs::write(&self.path, body)?;
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
    if let Ok(value) = std::env::var("CODEWHALE_HOME") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed).join("secrets").join("secrets.json"));
        }
    }
    Ok(FileKeyringStore::home_dir()?
        .join(".codewhale")
        .join("secrets")
        .join("secrets.json"))
}

fn legacy_deepseek_secrets_path() -> Result<PathBuf, SecretsError> {
    Ok(FileKeyringStore::home_dir()?
        .join(".deepseek")
        .join("secrets")
        .join("secrets.json"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SecretBackendSelection {
    File,
    System,
    Unknown,
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

    fn file_backed_default() -> Self {
        let path = FileKeyringStore::default_path()
            .unwrap_or_else(|_| PathBuf::from(".codewhale-secrets.json"));
        Self::new(Arc::new(FileKeyringStore::new(path)))
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
/// | `xiaomi-mimo` / `mimo` | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` |
/// | `novita` | `NOVITA_API_KEY` |
/// | `nvidia` / `nvidia-nim` / `nim` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, `DEEPSEEK_API_KEY` |
/// | `fireworks` | `FIREWORKS_API_KEY` |
/// | `siliconflow` / `siliconflow-cn` | `SILICONFLOW_API_KEY` |
/// | `arcee` / `arcee-ai` | `ARCEE_API_KEY` |
/// | `moonshot` / `kimi` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` |
/// | `sglang` | `SGLANG_API_KEY` |
/// | `vllm` | `VLLM_API_KEY` |
/// | `ollama` | `OLLAMA_API_KEY` |
/// | `openai` | `OPENAI_API_KEY` |
/// | `atlascloud` / `atlas` | `ATLASCLOUD_API_KEY` |
/// | `volcengine` / `ark` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` |
/// | `wanjie` / `wanjie-ark` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` |
///
/// Returns `None` if the provider is not recognised or none of its
/// candidate environment variables are set to a non-empty value.
#[must_use]
pub fn env_for(name: &str) -> Option<String> {
    let candidates: &[&str] = match name.to_ascii_lowercase().as_str() {
        "deepseek" => &["DEEPSEEK_API_KEY"],
        "openrouter" => &["OPENROUTER_API_KEY"],
        "xiaomi-mimo" | "xiaomi_mimo" | "xiaomimimo" | "mimo" | "xiaomi" => &[
            "XIAOMI_MIMO_TOKEN_PLAN_API_KEY",
            "MIMO_TOKEN_PLAN_API_KEY",
            "XIAOMI_MIMO_API_KEY",
            "XIAOMI_API_KEY",
            "MIMO_API_KEY",
        ],
        "novita" => &["NOVITA_API_KEY"],
        // NVIDIA NIM falls back to `DEEPSEEK_API_KEY` last because the
        // catalog endpoint accepts the same DeepSeek-issued key when no
        // dedicated NVIDIA token is set. This mirrors pre-v0.7 behaviour.
        "nvidia" | "nvidia-nim" | "nvidia_nim" | "nim" => {
            &["NVIDIA_API_KEY", "NVIDIA_NIM_API_KEY", "DEEPSEEK_API_KEY"]
        }
        "fireworks" | "fireworks-ai" => &["FIREWORKS_API_KEY"],
        "siliconflow" | "silicon-flow" | "silicon_flow" | "siliconflow-cn" | "siliconflow_cn"
        | "silicon-flow-cn" | "silicon_flow_cn" | "siliconflow-china" => &["SILICONFLOW_API_KEY"],
        "arcee" | "arcee-ai" | "arcee_ai" => &["ARCEE_API_KEY"],
        "moonshot" | "moonshot-ai" | "kimi" | "kimi-k2" => &["MOONSHOT_API_KEY", "KIMI_API_KEY"],
        "sglang" | "sg-lang" => &["SGLANG_API_KEY"],
        "vllm" | "v-llm" => &["VLLM_API_KEY"],
        "ollama" | "ollama-local" => &["OLLAMA_API_KEY"],
        "openai" => &["OPENAI_API_KEY"],
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    /// Serialise env-mutating tests: tests in this module poke
    /// `DEEPSEEK_API_KEY` etc., which is process-global.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
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
            "SILICONFLOW_API_KEY",
            "ARCEE_API_KEY",
            "SGLANG_API_KEY",
            "VLLM_API_KEY",
            "OLLAMA_API_KEY",
            "OPENAI_API_KEY",
            "ATLASCLOUD_API_KEY",
            "WANJIE_ARK_API_KEY",
            "WANJIE_API_KEY",
            "WANJIE_MAAS_API_KEY",
            "XIAOMI_MIMO_TOKEN_PLAN_API_KEY",
            "MIMO_TOKEN_PLAN_API_KEY",
            "XIAOMI_MIMO_API_KEY",
            "XIAOMI_API_KEY",
            "MIMO_API_KEY",
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
        unsafe { std::env::set_var("NVIDIA_NIM_API_KEY", "nim-key") };
        let secrets = Secrets::new(Arc::new(InMemoryKeyringStore::new()));
        assert_eq!(secrets.resolve("nvidia-nim").as_deref(), Some("nim-key"));
        assert_eq!(secrets.resolve("nvidia").as_deref(), Some("nim-key"));
        // Safety: env mutation guarded by env_lock().
        unsafe { std::env::remove_var("NVIDIA_NIM_API_KEY") };
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

        unsafe {
            std::env::set_var("XIAOMI_MIMO_API_KEY", "standard-key");
            std::env::set_var("XIAOMI_MIMO_TOKEN_PLAN_API_KEY", "token-plan-key");
        }
        assert_eq!(env_for("xiaomi-mimo").as_deref(), Some("token-plan-key"));
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
}
