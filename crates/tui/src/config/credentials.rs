//! Credential persistence: where API keys and provider metadata are
//! written (durable secret store vs config file), the save/clear/read
//! surfaces for provider credentials, and the config-file write helpers
//! they share.
//!
//! Extracted verbatim from `config.rs` (#5586); visibility is unchanged
//! and the parent re-exports every name so `crate::config::<name>` paths
//! keep resolving. Known follow-up (not changed by this move): the
//! `toml::from_str::<Config>` in `clear_active_provider_api_key_under_lock`
//! still parses on the caller's stack and should join the sheltered
//! parse-thread pattern when touched next.

use super::*;

pub fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        #[cfg(unix)]
        {
            // Tighten group/other bits on the parent dir as a hardening pass.
            // The dir lives under the user's home, so the chmod is best-effort:
            // filesystems that don't accept Unix permission bits (Docker
            // bind-mounts of NTFS, network shares, FAT, certain CI volumes —
            // see #897) return EPERM/ENOTSUP. The dir already exists by the
            // time we get here, so failing the whole save just because we
            // couldn't tighten perms strands the user mid-onboarding. Warn
            // loudly so a security-sensitive operator can still notice via
            // `RUST_LOG=warn`, then continue.
            if let Ok(meta) = fs::metadata(parent) {
                let mode = meta.permissions().mode();
                if mode & 0o077 != 0 {
                    let mut perms = meta.permissions();
                    perms.set_mode(mode & !0o077);
                    if let Err(err) = fs::set_permissions(parent, perms) {
                        tracing::warn!(
                            target: "codewhale::config",
                            path = %parent.display(),
                            error = %err,
                            "could not tighten parent dir permissions; \
                             filesystem may not support Unix chmod \
                             (Docker bind-mount, NTFS, network share). \
                             Continuing — the file will still be written."
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Write content to a config file with restrictive permissions (owner-only read/write).
/// On Unix this sets mode 0o600 before writing.
pub(super) fn write_config_file_secure(path: &Path, content: &str) -> Result<()> {
    codewhale_config::create_config_document(path, content)
}

/// Where a saved credential ended up. Returned by [`save_api_key`] so
/// the caller can show a confirmation message without leaking the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SavedCredential {
    /// Stored in the durable secret store. The config file contains only
    /// non-secret provider metadata and has any matching plaintext `api_key`
    /// entry removed. The `backend` label is the value of
    /// [`codewhale_secrets::Secrets::backend_name`] at write time so the toast
    /// text can name the actual backend (`"system keyring"`,
    /// `"file-based (~/.codewhale/secrets/)"`).
    KeyringAndConfigFile {
        /// `Secrets::backend_name()` at write time.
        backend: String,
        /// Absolute path to the credential-free config metadata file.
        path: PathBuf,
    },
    /// Stored in the Codewhale config file only under `cfg(test)` so unit tests
    /// without an explicitly isolated secret backend do not pollute the host
    /// credential store. Production save flows never automatically downgrade
    /// a failed secret-store write to plaintext.
    ConfigFile(PathBuf),
}

impl SavedCredential {
    /// Human-readable description for status / log output. Never
    /// includes the key value.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::KeyringAndConfigFile { backend, path } => {
                format!(
                    "secret store ({backend}); credential-free config metadata in {}",
                    path.display()
                )
            }
            Self::ConfigFile(path) => path.display().to_string(),
        }
    }
}

/// Resolve the config document for CREDENTIAL writes: api_key values,
/// `auth_mode` markers, and oauth/external-credential pointers.
///
/// Credentials are user-global — a key saved while working in one repo must be
/// visible from every other repo (#5045, #5193). The ambient
/// `CODEWHALE_CONFIG_PATH`/`DEEPSEEK_CONFIG_PATH` override can point at a
/// workspace-scoped document (`<repo>/.codewhale/config.toml`, plaintext and
/// easy to commit by accident), so credential writes that would land there are
/// rescoped to the user-global config instead. Non-credential settings keep
/// the ambient scoping, and callers that pass an explicit config path never
/// consult this resolver; a per-workspace destination stays possible only as
/// that kind of explicit opt-in.
fn credential_config_path() -> anyhow::Result<PathBuf> {
    let resolved = try_default_config_path()?;
    if !codewhale_config::config_path_is_workspace_scoped(&resolved) {
        return Ok(resolved);
    }
    let global = home_config_path()
        .context("Failed to resolve user-global config path: home directory not found.")?;
    tracing::info!(
        ambient = %resolved.display(),
        global = %global.display(),
        "rescoping credential write from workspace config to user-global config"
    );
    Ok(global)
}

/// Save the active provider's API key.
///
/// The selected durable secret backend is attempted first. On success the
/// config keeps only non-secret auth metadata and any older plaintext copy is
/// removed. When the secret-store write fails (OS permission denied, corrupt
/// or read-only file backend, etc.), the save fails loudly rather than writing
/// the key to plaintext `config.toml`.
///
/// Under `cfg(test)` the secret-store path is enabled only when the test sets
/// both an isolated `CODEWHALE_HOME` and an explicit backend, preventing unit
/// tests from touching the developer's real credential store.
pub fn save_api_key(api_key: &str) -> Result<SavedCredential> {
    save_root_api_key_for_secret_slot(api_key, "deepseek", true)
}

fn save_root_api_key_for_secret_slot(
    api_key: &str,
    secret_slot: &str,
    clear_deepseek_provider_slot: bool,
) -> Result<SavedCredential> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        anyhow::bail!("Refusing to save an empty API key.");
    }

    let path = credential_config_path().context("Failed to resolve config path for API key.")?;

    if let Some(secrets) = credential_secret_store() {
        // Same read-modify-write as the per-provider save below; hold the slot's
        // write lock across snapshot, store write, config write, and rollback.
        return crate::credentials::store::with_provider_write_lock(secret_slot, || {
            let prior_secret = secrets.get(secret_slot);
            match prior_secret.as_ref() {
                Ok(prior) => match secrets.set(secret_slot, trimmed) {
                    Ok(()) => {
                        if let Err(error) = save_root_api_key_metadata_without_plaintext(
                            &path,
                            clear_deepseek_provider_slot,
                        ) {
                            let current = secrets.get(secret_slot).map_err(|rollback| {
                        anyhow::anyhow!(
                            "{error}; additionally could not verify secret-store rollback for {secret_slot}: {rollback}"
                        )
                    })?;
                            if current.as_deref() == Some(trimmed) {
                                match prior {
                            Some(previous) => secrets.set(secret_slot, previous),
                            None => secrets.delete(secret_slot),
                        }
                        .map_err(|rollback| {
                            anyhow::anyhow!(
                                "{error}; additionally failed to restore prior secret-store state for {secret_slot}: {rollback}"
                            )
                        })?;
                            }
                            return Err(error);
                        }
                        codewhale_config::scrub_plaintext_api_keys_from_config_backup(&path)?;
                        let backend = secrets.backend_name().to_string();
                        log_sensitive_event(
                            "credential.save",
                            json!({
                                "backend": backend.clone(),
                                "config_path": path.display().to_string(),
                                "plaintext_config_fallback": false,
                            }),
                        );
                        Ok(SavedCredential::KeyringAndConfigFile { backend, path })
                    }
                    Err(err) => Err(plaintext_credential_fallback_refused("write", &path, &err)),
                },
                Err(error) => Err(plaintext_credential_fallback_refused(
                    "snapshot", &path, &error,
                )),
            }
        });
    }

    let path = save_api_key_to_config_file(trimmed)?;
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(&path)?;
    Ok(SavedCredential::ConfigFile(path))
}

fn plaintext_credential_fallback_refused(
    operation: &str,
    config_path: &Path,
    failure: &dyn std::fmt::Display,
) -> anyhow::Error {
    anyhow::anyhow!(
        "Secret storage {operation} failed: {failure}. Refusing to write the API key in plaintext to {}. Fix the configured secret backend and retry; Codewhale did not change that file.",
        codewhale_config::quote_os_path(config_path)
    )
}

/// The durable secret store for credential saves and logout-time deletes.
///
/// Under `cfg(test)` the store is only exposed when the test set both an
/// isolated `CODEWHALE_HOME` and an explicit backend, so unit tests can never
/// touch the developer's real credential store.
#[cfg(not(test))]
fn credential_secret_store() -> Option<codewhale_secrets::Secrets> {
    Some(codewhale_secrets::Secrets::auto_detect())
}

#[cfg(test)]
fn credential_secret_store() -> Option<codewhale_secrets::Secrets> {
    let isolated_home = codewhale_paths::codewhale_home_is_explicit();
    let explicit_backend = std::env::var_os("CODEWHALE_SECRET_BACKEND")
        .or_else(|| std::env::var_os("DEEPSEEK_SECRET_BACKEND"))
        .is_some_and(|value| !value.is_empty());
    (isolated_home && explicit_backend).then(codewhale_secrets::Secrets::auto_detect)
}

fn save_root_api_key_metadata_without_plaintext(
    config_path: &Path,
    clear_deepseek_provider_slot: bool,
) -> Result<()> {
    ensure_parent_dir(config_path)?;
    crate::config_persistence::mutate_config_document(config_path, |doc| {
        crate::config_persistence::set_document_value(doc, &["auth_mode"], "api_key")?;
        if !doc.contains_key("default_text_model") {
            crate::config_persistence::set_document_value(
                doc,
                &["default_text_model"],
                DEFAULT_TEXT_MODEL,
            )?;
        }
        if !doc.contains_key("reasoning_effort") {
            crate::config_persistence::set_document_value(doc, &["reasoning_effort"], "max")?;
        }
        crate::config_persistence::unset_document_value(doc, &["api_key"])?;
        if clear_deepseek_provider_slot {
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "deepseek", "api_key"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "deepseek-cn", "api_key"],
            )?;
        }
        Ok(())
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))
}

/// Write the `api_key` slot directly to `config.toml`.
fn save_api_key_to_config_file(api_key: &str) -> Result<PathBuf> {
    let config_path =
        credential_config_path().context("Failed to resolve config path for API key.")?;

    ensure_parent_dir(&config_path)?;

    if config_path.exists() {
        // TOML-aware upsert. The old line scan keyed off
        // `existing.contains("api_key")`, so a comment that merely mentioned
        // api_key made it skip the insert entirely; editing the document
        // replaces or inserts the real key and keeps user comments.
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["api_key"], api_key)?;
            crate::config_persistence::set_document_value(doc, &["auth_mode"], "api_key")
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    } else {
        // Create new minimal config
        let content = format!(
            r#"# codewhale Configuration
# Set provider credentials in this file or via environment variables.
# See /links in the TUI for provider-specific credential pages.

api_key = "{api_key}"
auth_mode = "api_key"

# Base URL (default: https://api.deepseek.com/beta)
# Set https://api.deepseek.com to opt out of beta features.
# base_url = "https://api.deepseek.com/beta"

# Default model
default_text_model = "{DEFAULT_TEXT_MODEL}"

# Thinking mode (DeepSeek V4 reasoning effort):
# "off" | "low" | "medium" | "high" | "max"
# Shift+Tab in the TUI cycles between off / high / max.
reasoning_effort = "max"
"#
        );
        crate::config_persistence::write_config_toml_atomic(&config_path, &content)
            .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    }

    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "config_path": config_path.display().to_string(),
        }),
    );

    Ok(config_path)
}

/// Check if the active provider has any API key configured anywhere the
/// runtime can resolve it.
///
/// The default secret store is file-backed and prompt-free. An OS credential
/// store is queried only when the user explicitly selects the system backend.
///
/// Used by the TUI app constructor to decide whether to gate
/// the user behind the in-TUI api-key onboarding screen — getting
/// this wrong made users get prompted for credentials in situations
/// where normal env/config auth was already available.
pub fn has_api_key(config: &Config) -> bool {
    has_api_key_for(config, config.api_provider())
}

pub(super) fn provider_uses_oauth_credentials(config: &Config, provider: ApiProvider) -> bool {
    !auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref())
        && !config.provider_uses_custom_endpoint(provider)
        && (provider == ApiProvider::OpenaiCodex
            || (provider == ApiProvider::Moonshot
                && config
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_kimi_imported_token))
            || (provider == ApiProvider::Xai
                && config
                    .provider_config_for(provider)
                    .is_some_and(provider_config_uses_xai_oauth)))
}

/// The environment variable name a provider route explicitly binds via
/// `[providers.<name>] api_key_env`, when credentials are bound to the active
/// endpoint. `None` when the route declares no binding.
pub(super) fn bound_provider_api_key_env_name(
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    if !config.config_credentials_are_bound_to_provider_endpoint(provider) {
        return None;
    }
    config
        .provider_config_for(provider)
        .and_then(|entry| entry.api_key_env.as_deref())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

pub(super) fn provider_config_env_api_key(
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    let env_name = bound_provider_api_key_env_name(config, provider)?;
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

#[must_use]
pub fn active_provider_has_config_api_key(config: &Config) -> bool {
    let provider = config.api_provider();
    if auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref()) {
        return false;
    }
    let custom_endpoint = config.provider_uses_custom_endpoint(provider);

    if provider == ApiProvider::Moonshot
        && !custom_endpoint
        && config
            .provider_config_for(provider)
            .is_some_and(provider_config_uses_kimi_imported_token)
    {
        return false;
    }
    if provider == ApiProvider::OpenaiCodex && !custom_endpoint {
        // The persistent Codex login is the OAuth credential file, analogous to
        // a stored config key. Token env overrides are scored separately by
        // active_provider_has_env_api_key.
        let path = crate::oauth::auth_file_path();
        return config
            .external_credential_read_grant(
                provider,
                codewhale_config::ExternalCredentialSource::CodexCli,
                &path,
            )
            .is_ok_and(|grant| crate::oauth::stored_credentials_present(&grant));
    }
    if !custom_endpoint
        && matches!(provider, ApiProvider::Huggingface)
        && std::env::var("HUGGINGFACE_API_KEY")
            .or_else(|_| std::env::var("HF_TOKEN"))
            .is_ok_and(|k| !k.trim().is_empty())
    {
        return true;
    }

    if config.config_credentials_are_bound_to_provider_endpoint(provider)
        && config
            .provider_config_string_with_runtime_fallback(provider, |entry| entry.api_key.clone())
            .is_some_and(|key| {
                classify_config_api_key_value(&key) == ConfigApiKeyValueKind::Literal
            })
    {
        return true;
    }
    if !config.should_skip_secret_store_for_provider(provider)
        && provider_secret_store_api_key(config, provider).is_some()
    {
        return true;
    }

    matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        && config.config_credentials_are_bound_to_provider_endpoint(provider)
        && config
            .api_key
            .as_ref()
            .is_some_and(|key| classify_config_api_key_value(key) == ConfigApiKeyValueKind::Literal)
}

#[must_use]
pub fn active_provider_has_env_api_key(config: &Config) -> bool {
    let provider = config.api_provider();
    if auth_mode_disables_api_key(config.auth_mode_for_provider(provider).as_deref()) {
        return false;
    }
    (!provider_uses_oauth_credentials(config, provider)
        && explicit_cli_api_key_override().is_some())
        || provider_config_env_api_key(config, provider).is_some()
        || (!config.should_skip_secret_store_for_provider(provider)
            && provider_env_api_key(provider).is_some())
}

#[must_use]
pub fn active_provider_uses_env_only_api_key(config: &Config) -> bool {
    active_provider_has_env_api_key(config) && !active_provider_has_config_api_key(config)
}

/// A key saved in the user-global config file stays visible even when this
/// process loaded a DIFFERENT config (e.g. an explicit workspace `--config`
/// path). Credentials are user-global: a workspace override may select a
/// different route, but it must never make a global credential appear locked.
///
/// Bounded, read-only, non-migrating: parses the default config file's raw
/// provider table directly (never runs legacy migration, never opens a
/// write-capable backend). Returns the key only when it reads as a real
/// literal, not a placeholder.
struct UserGlobalConfigCache {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    json: serde_json::Value,
}

fn user_global_config_json() -> Option<serde_json::Value> {
    static CACHE: Mutex<Option<UserGlobalConfigCache>> = Mutex::new(None);
    let path = codewhale_config::default_config_path().ok()?;
    let meta = fs::metadata(&path).ok()?;
    let modified = meta.modified().ok();
    let len = meta.len();
    let mut guard = CACHE.lock().ok()?;
    if let Some(cached) = guard.as_ref()
        && cached.path == path
        && cached.modified == modified
        && cached.len == len
    {
        return Some(cached.json.clone());
    }
    let text = fs::read_to_string(&path).ok()?;
    let doc: codewhale_config::ConfigToml = toml::from_str(&text).ok()?;
    let json = serde_json::to_value(&doc).ok()?;
    *guard = Some(UserGlobalConfigCache {
        path,
        modified,
        len,
        json: json.clone(),
    });
    Some(json)
}

pub(super) fn user_global_config_api_key(provider: ApiProvider) -> Option<String> {
    if provider == ApiProvider::Custom {
        // Custom providers are per-config by nature; the probe applies to
        // built-in ids whose keys are saved under the user-global file.
        return None;
    }
    let json = user_global_config_json()?;
    let provider_config_key = provider.metadata().map_or_else(
        || provider.as_str(),
        |metadata| metadata.provider_config_key(),
    );
    let key = json
        .get("providers")?
        .get(provider_config_key)?
        .get("api_key")?
        .as_str()?;
    let key = key.trim();
    if key.is_empty() || classify_config_api_key_value(key) != ConfigApiKeyValueKind::Literal {
        return None;
    }
    Some(key.to_string())
}

/// Check whether the given provider has any usable API key — via env var,
/// provider/root config. Used by the `/provider` picker to decide whether to
/// prompt for a key inline.
#[must_use]
pub fn has_api_key_for(config: &Config, provider: ApiProvider) -> bool {
    credential_resolve::resolve_credential_source(config, provider).is_present()
}

impl Config {
    /// Resolve one coherent Codex OAuth snapshot. The bearer and account id
    /// must come from the same secure file handle; opening the external JSON a
    /// second time could pair identities across an atomic owner refresh or a
    /// hostile path swap.
    pub(crate) fn codex_credentials(&self) -> Result<crate::oauth::CodexCredentials> {
        if let Some(credentials) = crate::oauth::credentials_from_env() {
            return Ok(credentials);
        }
        anyhow::ensure!(
            self.api_provider() == ApiProvider::OpenaiCodex
                && !self.provider_uses_custom_endpoint(ApiProvider::OpenaiCodex),
            "Codex OAuth credentials are only available on the official OpenAI Codex route"
        );
        let path = crate::oauth::auth_file_path();
        let grant = self.external_credential_read_grant(
            ApiProvider::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
            &path,
        )?;
        crate::oauth::get_credentials(&grant)
    }

    /// ChatGPT account id for the already-selected Codex route. Environment
    /// metadata remains independent; the external file is read only when the
    /// exact provider/source/path consent tuple is valid.
    #[cfg(test)]
    pub(crate) fn codex_account_id(&self) -> Option<String> {
        self.codex_credentials()
            .ok()
            .and_then(|credentials| credentials.account_id)
    }
}

/// Whether a provider counts as "configured" for the default `/provider`
/// and `/model` manager views (#3830). Shared by both pickers so "what shows
/// up without browsing the full catalog" stays a single definition.
/// Self-hosted providers (Ollama/Sglang/Vllm) report `has_key = true`
/// unconditionally in [`has_api_key_for`] since they don't require auth to
/// route to — that's correct for routing, but wrong for "did the user set
/// this up," so a self-hosted provider only qualifies via an explicit
/// `[providers.<name>]` entry or being active, never via `has_key` alone
/// (otherwise every self-hosted provider type would always show up).
#[must_use]
pub(crate) fn provider_is_configured(
    provider: ApiProvider,
    is_active: bool,
    has_key: bool,
    configured: Option<&ProviderConfig>,
    is_named_custom_entry: bool,
) -> bool {
    // A *named* custom provider entry (one the user actually added) always
    // counts. The unconfigured `Custom` placeholder row that fills the slot
    // when no custom provider exists yet is not itself "configured" — it's
    // the catalog's invitation to add one.
    if is_active || is_named_custom_entry {
        return true;
    }
    if configured.is_some_and(provider_config_is_explicit) {
        return true;
    }
    if provider.is_self_hosted() {
        return false;
    }
    has_key
}

/// Convenience wrapper around [`provider_is_configured`] for callers that
/// just want "is this provider configured given the active one," without
/// the provider picker's multi-row named-custom-provider bookkeeping
/// (`is_named_custom_entry`) — e.g. the `/model` picker (#3830), which only
/// ever resolves the single, currently-selected `Custom` slot via
/// [`Config::provider_config_for`], the same way model/route resolution
/// does everywhere else.
#[must_use]
pub(crate) fn provider_is_configured_for_active(
    config: &Config,
    provider: ApiProvider,
    active: ApiProvider,
) -> bool {
    provider_is_configured(
        provider,
        provider == active,
        has_api_key_for(config, provider),
        config.provider_config_for(provider),
        false,
    )
}

/// True when a `[providers.<name>]` table entry has any field the user would
/// have had to set explicitly — base URL, model, auth, etc. Used by
/// [`provider_is_configured`]: merely existing in the
/// (always-`Some`-once-any-provider-is-configured) `ProvidersConfig` struct
/// isn't enough, since untouched providers still resolve to a
/// `ProviderConfig::default()` there.
pub(super) fn provider_config_is_explicit(entry: &ProviderConfig) -> bool {
    let non_empty = |value: Option<&String>| value.is_some_and(|value| !value.trim().is_empty());

    non_empty(entry.api_key.as_ref())
        || non_empty(entry.base_url.as_ref())
        || non_empty(entry.model.as_ref())
        || non_empty(entry.auth_mode.as_ref())
        || entry
            .auth
            .as_ref()
            .is_some_and(|auth| auth.validate().is_ok())
        || entry.context_window.is_some()
        || non_empty(entry.mode.as_ref())
        || entry.max_concurrency.is_some()
        || entry.http_headers.as_ref().is_some_and(|headers| {
            headers
                .iter()
                .any(|(name, value)| !name.trim().is_empty() && !value.trim().is_empty())
        })
        || non_empty(entry.path_suffix.as_ref())
        || non_empty(entry.reasoning_stream_style.as_ref())
        || entry.insecure_skip_tls_verify.is_some()
        || non_empty(entry.kind.as_ref())
        || non_empty(entry.api_key_env.as_ref())
        || entry.external_credentials.is_some()
        || non_empty(entry.oauth_credential_generation.as_ref())
}

/// Save an API key to the appropriate place for the given provider.
/// DeepSeek goes through [`save_api_key`]. Other providers write
/// `[providers.<name>] api_key = "..."` to `~/.codewhale/config.toml`.
/// Returns the config file path.
#[cfg(test)]
pub fn save_api_key_for(provider: ApiProvider, api_key: &str) -> Result<PathBuf> {
    match save_api_key_for_identity(
        &ProviderIdentity {
            provider,
            key: provider.as_str().to_string(),
            exact_id: Some(provider.as_str().to_string()),
            migrated_legacy_ollama_cloud_route: false,
        },
        &Config {
            provider: Some(provider.as_str().to_string()),
            ..Config::default()
        },
        api_key,
    )? {
        SavedCredential::KeyringAndConfigFile { path, .. } | SavedCredential::ConfigFile(path) => {
            Ok(path)
        }
    }
}

/// Save an API key for the given provider identity and return where the
/// credential actually landed ([`SavedCredential`]) so callers can state the
/// true destination — the durable secret store plus credential-free config
/// metadata, or (tests only) the plaintext config file (#5195).
pub(crate) fn save_api_key_for_identity(
    identity: &ProviderIdentity,
    route_config: &Config,
    api_key: &str,
) -> Result<SavedCredential> {
    if identity.provider == ApiProvider::Xai {
        return codewhale_config::with_xai_oauth_revocation_transaction(|| {
            save_api_key_for_identity_unlocked(identity, route_config, api_key)
        });
    }
    save_api_key_for_identity_unlocked(identity, route_config, api_key)
}

fn save_api_key_for_identity_unlocked(
    identity: &ProviderIdentity,
    route_config: &Config,
    api_key: &str,
) -> Result<SavedCredential> {
    let provider = identity.provider;
    if provider == ApiProvider::OpenaiCodex {
        anyhow::bail!(
            "OpenAI Codex uses OAuth. Run `codex login`, then grant exact read-only access with `codewhale auth external-consent --provider openai-codex --mode read-only`, or set OPENAI_CODEX_ACCESS_TOKEN for this process; Codewhale does not store an API key for this provider."
        );
    }
    let is_legacy_literal_custom = provider == ApiProvider::Custom
        && identity.key.trim() == ApiProvider::Custom.as_str()
        && identity.persisted_id().is_none();
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        return save_api_key(api_key);
    }
    if is_legacy_literal_custom {
        return save_root_api_key_for_secret_slot(api_key, "custom", false);
    }

    let api_key = api_key.trim();
    anyhow::ensure!(!api_key.is_empty(), "Refusing to save an empty API key.");

    let config_path =
        credential_config_path().context("Failed to resolve config path for provider API key.")?;
    ensure_parent_dir(&config_path)?;

    let key_inside = if provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(provider).context("provider api key table")?
    };
    // A legacy, manually-selected Kimi CLI import implicitly routed Moonshot
    // traffic to Kimi Code. Once the user replaces that import with the
    // supported API-key route, persist the endpoint before changing auth_mode
    // so the key is not silently sent to the ordinary Moonshot endpoint.
    // Respect an explicit user-owned endpoint.
    let pin_kimi_code_base_url = provider == ApiProvider::Moonshot
        && route_config
            .provider_config_for(provider)
            .is_some_and(|entry| {
                provider_config_uses_kimi_imported_token(entry)
                    && entry
                        .base_url
                        .as_deref()
                        .is_none_or(|base_url| base_url.trim().is_empty())
            });

    if !route_config.should_skip_secret_store_for_provider(provider)
        && let Some(secrets) = credential_secret_store()
    {
        let secret_slot = provider_secret_store_slot(provider);
        // Snapshot -> write -> config-write -> rollback is a read-modify-write.
        // Hold this provider's credential write lock across the whole sequence
        // so a concurrent save or logout on the same slot cannot interleave and
        // leave the secret store and the config document disagreeing. This is
        // the `modify`-is-the-only-write-path rule ported from pi-mono; see
        // `crate::credentials::store`.
        return crate::credentials::store::with_provider_write_lock(secret_slot, || {
            let prior_secret = secrets.get(secret_slot);
            match prior_secret.as_ref() {
                Ok(prior) => match secrets.set(secret_slot, api_key) {
                    Ok(()) => {
                        let config_result = crate::config_persistence::mutate_config_document(
                            &config_path,
                            |doc| {
                                if pin_kimi_code_base_url {
                                    crate::config_persistence::set_document_value(
                                        doc,
                                        &["providers", key_inside, "base_url"],
                                        DEFAULT_KIMI_CODE_BASE_URL,
                                    )?;
                                }
                                crate::config_persistence::set_document_value(
                                    doc,
                                    &["providers", key_inside, "auth_mode"],
                                    "api_key",
                                )?;
                                crate::config_persistence::unset_document_value(
                                    doc,
                                    &["providers", key_inside, "external_credentials"],
                                )?;
                                if provider == ApiProvider::Xai {
                                    crate::config_persistence::unset_document_value(
                                        doc,
                                        &["providers", key_inside, "oauth_credential_generation"],
                                    )?;
                                }
                                crate::config_persistence::unset_document_value(
                                    doc,
                                    &["providers", key_inside, "api_key"],
                                )?;
                                Ok(())
                            },
                        )
                        .with_context(|| {
                            format!("Failed to write config to {}", config_path.display())
                        });
                        if let Err(error) = config_result {
                            let current = secrets.get(secret_slot).map_err(|rollback| {
                        anyhow::anyhow!(
                            "{error}; additionally could not verify secret-store rollback for {secret_slot}: {rollback}"
                        )
                    })?;
                            if current.as_deref() == Some(api_key) {
                                match prior {
                            Some(previous) => secrets.set(secret_slot, previous),
                            None => secrets.delete(secret_slot),
                        }
                        .map_err(|rollback| {
                            anyhow::anyhow!(
                                "{error}; additionally failed to restore prior secret-store state for {secret_slot}: {rollback}"
                            )
                        })?;
                            }
                            return Err(error);
                        }
                        codewhale_config::scrub_plaintext_api_keys_from_config_backup(
                            &config_path,
                        )?;
                        let backend = secrets.backend_name().to_string();
                        log_sensitive_event(
                            "credential.save",
                            json!({
                                "backend": backend.clone(),
                                "provider": identity.key,
                                "config_path": config_path.display().to_string(),
                                "plaintext_config_fallback": false,
                            }),
                        );
                        Ok(SavedCredential::KeyringAndConfigFile {
                            backend,
                            path: config_path,
                        })
                    }
                    Err(err) => Err(plaintext_credential_fallback_refused(
                        "write",
                        &config_path,
                        &err,
                    )),
                },
                Err(error) => Err(plaintext_credential_fallback_refused(
                    "snapshot",
                    &config_path,
                    &error,
                )),
            }
        });
    }

    // Edit the `[providers.<name>]` table in place so unrelated sections,
    // comments, and formatting survive the write.
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        if pin_kimi_code_base_url {
            crate::config_persistence::set_document_value(
                doc,
                &["providers", key_inside, "base_url"],
                DEFAULT_KIMI_CODE_BASE_URL,
            )?;
        }
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "auth_mode"],
            "api_key",
        )?;
        crate::config_persistence::unset_document_value(
            doc,
            &["providers", key_inside, "external_credentials"],
        )?;
        if provider == ApiProvider::Xai {
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", key_inside, "oauth_credential_generation"],
            )?;
        }
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "api_key"],
            api_key,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    log_sensitive_event(
        "credential.save",
        json!({
            "backend": "config_file",
            "provider": identity.key,
            "config_path": config_path.display().to_string(),
        }),
    );
    codewhale_config::scrub_plaintext_api_keys_from_config_backup(&config_path)?;

    Ok(SavedCredential::ConfigFile(config_path))
}

/// Persist a default model for `provider` via the comment-preserving config
/// path used by guided provider setup (#3875). DeepSeek writes root
/// `default_text_model`; other hosted providers write `[providers.<name>] model`.
pub(crate) fn save_provider_model_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    model: &str,
) -> Result<PathBuf> {
    let provider = identity.provider;
    let model = model.trim();
    anyhow::ensure!(!model.is_empty(), "model cannot be empty");

    let config_path =
        try_default_config_path().context("Failed to resolve config path for provider model.")?;
    ensure_parent_dir(&config_path)?;

    let is_legacy_literal_custom = provider == ApiProvider::Custom
        && identity.key.trim() == ApiProvider::Custom.as_str()
        && identity.persisted_id().is_none();
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN)
        || is_legacy_literal_custom
    {
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::set_document_value(doc, &["default_text_model"], model)
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        return Ok(config_path);
    }

    let key_inside = if provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(provider).context("provider model table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "model"],
            model,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Persist a guided-setup endpoint choice into the provider's own
/// `[providers.<name>] base_url` (#4526).
///
/// Deliberately narrow: it never touches the root `base_url`, another
/// provider's table, or any other key, so a billing-route choice cannot
/// repoint an unrelated route.
pub(crate) fn save_provider_base_url_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    base_url: &str,
) -> Result<PathBuf> {
    let base_url = base_url.trim();
    anyhow::ensure!(!base_url.is_empty(), "base URL cannot be empty");
    let config_path = try_default_config_path()
        .context("Failed to resolve config path for provider base URL.")?;
    ensure_parent_dir(&config_path)?;
    let key_inside = if identity.provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(identity.provider).context("provider base URL table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "base_url"],
            base_url,
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Persist a guided-setup context-window choice without replacing the user's
/// surrounding TOML comments or formatting.
pub(crate) fn save_provider_context_window_for_identity(
    identity: &ProviderIdentity,
    _route_config: &Config,
    context_window: u32,
) -> Result<PathBuf> {
    anyhow::ensure!(context_window > 0, "context window must be greater than 0");
    let config_path = try_default_config_path()
        .context("Failed to resolve config path for provider context window.")?;
    ensure_parent_dir(&config_path)?;
    let key_inside = if identity.provider == ApiProvider::Custom {
        let key = identity.key.trim();
        anyhow::ensure!(!key.is_empty(), "custom provider id cannot be empty");
        key
    } else {
        provider_config_key(identity.provider).context("provider context window table")?
    };
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "context_window"],
            i64::from(context_window),
        )
    })
    .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
    Ok(config_path)
}

/// Persist an explicitly confirmed read-only external credential grant and
/// update the live mirror only after the comment-preserving disk mutation
/// succeeds. This function never inspects the external path.
pub(crate) fn persist_external_credential_consent_for_at(
    config_path: Option<&Path>,
    live_config: &mut Config,
    provider: ApiProvider,
    consent_provider: codewhale_config::ProviderKind,
    source: codewhale_config::ExternalCredentialSource,
    path: &Path,
) -> Result<PathBuf> {
    let expected = match provider {
        ApiProvider::OpenaiCodex => (
            codewhale_config::ProviderKind::OpenaiCodex,
            codewhale_config::ExternalCredentialSource::CodexCli,
        ),
        ApiProvider::Xai => (
            codewhale_config::ProviderKind::Xai,
            codewhale_config::ExternalCredentialSource::GrokCli,
        ),
        _ => anyhow::bail!(
            "{} has no supported external credential owner",
            provider.as_str()
        ),
    };
    anyhow::ensure!(
        (consent_provider, source) == expected,
        "external credential owner does not match provider {}",
        provider.as_str()
    );
    let path = codewhale_config::resolve_external_credential_path(path)?;
    let path_value = path.to_str().context(
        "external credential path cannot be persisted losslessly because it is not valid UTF-8",
    )?;
    let config_path = match config_path {
        Some(path) => path.to_path_buf(),
        None => credential_config_path()
            .context("Failed to resolve config path for external credential consent.")?,
    };
    ensure_parent_dir(&config_path)?;
    let key_inside = provider_config_key(provider).context("external credential provider key")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::set_document_value(
            doc,
            &["providers", key_inside, "auth_mode"],
            "oauth",
        )?;
        let prefix = &["providers", key_inside, "external_credentials"];
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "access"],
            "read_only",
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "provider"],
            consent_provider.as_str(),
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "source"],
            source.as_str(),
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "path"],
            path_value,
        )?;
        crate::config_persistence::set_document_value(
            doc,
            &[prefix[0], prefix[1], prefix[2], "consent_version"],
            i64::from(codewhale_config::EXTERNAL_CREDENTIAL_CONSENT_VERSION),
        )
    })
    .with_context(|| {
        format!(
            "Failed to write config to {}",
            codewhale_config::quote_os_path(&config_path)
        )
    })?;
    live_config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry = live_config.provider_config_for_mut(provider);
    entry.auth_mode = Some("oauth".to_string());
    entry.external_credentials = Some(codewhale_config::ExternalCredentialConsentToml::read_only(
        consent_provider,
        source,
        path,
    ));
    Ok(config_path)
}

/// Revoke one provider's external-file access without inspecting that file.
pub(crate) fn revoke_external_credential_consent_for_at(
    config_path: Option<&Path>,
    live_config: &mut Config,
    provider: ApiProvider,
) -> Result<PathBuf> {
    anyhow::ensure!(
        matches!(provider, ApiProvider::OpenaiCodex | ApiProvider::Xai),
        "{} has no supported external credential owner",
        provider.as_str()
    );
    let config_path = match config_path {
        Some(path) => path.to_path_buf(),
        None => credential_config_path()
            .context("Failed to resolve config path for external credential consent.")?,
    };
    ensure_parent_dir(&config_path)?;
    let key_inside = provider_config_key(provider).context("external credential provider key")?;
    crate::config_persistence::mutate_config_document(&config_path, |doc| {
        crate::config_persistence::unset_document_value(
            doc,
            &["providers", key_inside, "external_credentials"],
        )?;
        Ok(())
    })
    .with_context(|| {
        format!(
            "Failed to write config to {}",
            codewhale_config::quote_os_path(&config_path)
        )
    })?;
    live_config
        .provider_config_for_mut(provider)
        .external_credentials = None;
    Ok(config_path)
}

pub(crate) fn provider_config_key(provider: ApiProvider) -> Result<&'static str> {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        anyhow::bail!("DeepSeek stores auth at the root config level");
    }
    provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
        .context("provider config key")
}

pub(super) fn provider_config_table_name(provider: ApiProvider) -> Result<String> {
    Ok(format!("providers.{}", provider_config_key(provider)?))
}

pub(super) fn provider_env_api_key(provider: ApiProvider) -> Option<String> {
    if provider == ApiProvider::Huggingface {
        return std::env::var("HUGGINGFACE_API_KEY")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                std::env::var("HF_TOKEN")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
            });
    }

    provider.env_vars().iter().find_map(|var| {
        std::env::var(var)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}

/// Canonical durable-credential slot shared with the CLI dispatcher.
pub(super) fn provider_secret_store_slot(provider: ApiProvider) -> &'static str {
    match provider {
        // TUI compatibility variants share the canonical CLI provider slots.
        ApiProvider::DeepseekCN => "deepseek",
        // Shared-account families (SiliconFlow China, the four Model Studio
        // variants) collapse onto one slot via ProviderKind::secret_store_slot.
        _ => provider
            .kind()
            .map_or_else(|| provider.as_str(), |kind| kind.secret_store_slot()),
    }
}

/// Whether the secret-store save marker (`auth_mode = "api_key"` with no
/// config literal, written by the save path) exists for `provider` or for any
/// provider sharing its durable credential slot.
///
/// One Model Studio account authenticates all four plan/dialect variants, so
/// saving a key on `modelstudio-token-plan` marks only that variant's config
/// table; the sibling variants must still treat the family slot as saved.
pub(super) fn secret_slot_save_marker_on_shared_slot(
    config: &Config,
    provider: ApiProvider,
) -> bool {
    let slot = provider_secret_store_slot(provider);
    ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .filter(|candidate| provider_secret_store_slot(*candidate) == slot)
        .any(|candidate| {
            config
                .provider_config_for(candidate)
                .is_some_and(|entry| auth_mode_requires_api_key(entry.auth_mode.as_deref()))
        })
}

/// Read only the durable secret-store layer (no environment fallback).
///
/// This keeps `config -> secret store -> env` precedence explicit in the TUI
/// and lets status surfaces distinguish a saved key from an ambient export.
pub(crate) fn provider_secret_store_api_key(
    config: &Config,
    provider: ApiProvider,
) -> Option<String> {
    provider_secret_store_api_key_with_mode(config, provider, false)
}

pub(super) fn provider_secret_store_api_key_with_mode(
    config: &Config,
    provider: ApiProvider,
    read_only: bool,
) -> Option<String> {
    // Keep the named-custom exclusion at the credential boundary itself.
    // Callers also use this policy to avoid unnecessary keyring probes, but a
    // future caller must not be able to read the legacy `custom` slot for an
    // arbitrary `[providers.<name>]` endpoint by omitting that outer guard.
    if config.should_skip_secret_store_for_provider(provider) {
        return None;
    }

    // Unit tests must never inspect the developer's real credential store.
    // Secret-store regressions opt in with an isolated CODEWHALE_HOME and an
    // explicit backend, matching the secrets crate's own test discipline.
    #[cfg(test)]
    if !codewhale_paths::codewhale_home_is_explicit()
        || std::env::var_os("CODEWHALE_SECRET_BACKEND").is_none()
    {
        return None;
    }

    let secrets = if read_only {
        codewhale_secrets::Secrets::auto_detect_read_only()
    } else {
        codewhale_secrets::Secrets::auto_detect()
    };
    // Read through the credential-store trait so every read of a durable slot
    // goes through one adapter (`crate::credentials::store`), and the value is
    // carried as a type-tagged `Credential` rather than a bare String that can
    // drift into a log line.
    let store =
        crate::credentials::store::SecretStoreCredentials::new(secrets, known_secret_store_slots());
    let primary = store
        .read(provider_secret_store_slot(provider))
        .ok()
        .flatten()
        .map(|credential| credential.expose_secret().to_string());
    if primary.is_some() {
        return primary;
    }

    // The old local identity owned the hosted slot only when the live config
    // selected the exact Ollama Cloud route. Never apply this fallback to a
    // neighboring/custom endpoint or to an explicit new `ollama-cloud`
    // selection, and never write/copy/delete either slot while resolving.
    (provider == ApiProvider::OllamaCloud && config.selects_legacy_ollama_cloud_route())
        .then(|| {
            store
                .read(ApiProvider::Ollama.as_str())
                .ok()
                .flatten()
                .map(|credential| credential.expose_secret().to_string())
        })
        .flatten()
}

/// Every durable credential slot CodeWhale knows how to write.
///
/// The backing keyring exposes no key enumeration, so
/// [`crate::credentials::store::SecretStoreCredentials::list`] is given the
/// slot names to probe. Deduplicated because shared-account families collapse
/// several providers onto one slot.
fn known_secret_store_slots() -> Vec<String> {
    let mut slots: Vec<String> = ApiProvider::all()
        .iter()
        .copied()
        .chain(std::iter::once(ApiProvider::DeepseekCN))
        .map(|provider| provider_secret_store_slot(provider).to_string())
        .collect();
    slots.sort();
    slots.dedup();
    slots
}

/// The shadowing warning for a config-file `api_key` that wins over a live
/// secret-store credential, if both exist (#5194).
///
/// The config file intentionally outranks the secret store in the read
/// chain, but a shadowed slot is invisible: the user rotates the key with
/// `codewhale auth set` and nothing changes, because the stale plaintext
/// copy still wins. Mirror the fleet-roster shadowing rule (#5098):
/// precedence is normal, but it must be VISIBLE. The message names both
/// sources, which one won, and the command that resolves the shadow.
/// Split from [`warn_on_config_api_key_shadowing`] so the decision is
/// testable without capturing tracing output.
pub(super) fn config_api_key_shadow_warning(
    config: &Config,
    provider: ApiProvider,
    config_source: &str,
) -> Option<String> {
    if config.should_skip_secret_store_for_provider(provider) {
        return None;
    }
    provider_secret_store_api_key_with_mode(config, provider, true).map(|_| {
        let slot = provider_secret_store_slot(provider);
        let id = provider.as_str();
        format!(
            "both {config_source} in the config file and secret-store slot \"{slot}\" \
             hold a credential for provider {id}; the config-file key won. Run \
             `codewhale auth set --provider {id}` to move the key into the secret store \
             and strip the plaintext copy, or remove the config-file api_key."
        )
    })
}

/// Emit the #5194 shadowing warning at most once per provider slot per
/// process: credential resolution runs on every request, and a repeating
/// warning is noise, not signal.
pub(super) fn warn_on_config_api_key_shadowing(
    config: &Config,
    provider: ApiProvider,
    config_source: &str,
) {
    let Some(message) = config_api_key_shadow_warning(config, provider, config_source) else {
        return;
    };
    static WARNED_SLOTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<&'static str>>,
    > = std::sync::OnceLock::new();
    let mut warned = WARNED_SLOTS
        .get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !warned.insert(provider_secret_store_slot(provider)) {
        return;
    }
    drop(warned);
    tracing::warn!("{message}");
}

/// The model this launch was explicitly asked for, if any.
///
/// The `codewhale` dispatcher forwards `--model` to this binary as
/// `CODEWHALE_MODEL` (with the legacy `DEEPSEEK_MODEL` alias), so an explicit
/// flag and an explicit shell export are the same signal here: *the user named
/// a model for this run*. That has to outrank the remembered per-provider
/// selection in `settings.toml`, which is a convenience memory of the last
/// `/model` pick — never a reason to run something the user did not ask for
/// (v0.9.1 kimi-k3 dogfood report).
pub(crate) fn explicit_launch_model_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_MODEL", "DEEPSEEK_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// The provider this launch was explicitly asked for, if any.
///
/// An environment/CLI override is a one-run instruction and must outrank the
/// user's saved startup default. A provider merely named in config.toml is a
/// seed instead: the user can deliberately replace that seed from `/model`.
pub(crate) fn explicit_launch_provider_override() -> Option<String> {
    codewhale_env_var("CODEWHALE_PROVIDER", "DEEPSEEK_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn explicit_cli_api_key_override() -> Option<String> {
    (cli_api_key_source().as_deref() == Some("cli"))
        .then(|| {
            std::env::var(codewhale_config::CLI_API_KEY_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .flatten()
}

pub(crate) fn cli_api_key_source() -> Option<String> {
    codewhale_env_var(
        codewhale_config::CLI_API_KEY_SOURCE_ENV,
        codewhale_config::LEGACY_CLI_API_KEY_SOURCE_ENV,
    )
    .ok()
}

pub(super) fn missing_provider_api_key_message(provider: ApiProvider) -> Result<String> {
    let credential_hint = provider
        .credential_url()
        .map(|url| format!(" Get a key: {url}."))
        .unwrap_or_default();
    Ok(format!(
        "{} API key not found.{} Run 'codewhale auth set --provider {}', set {}, or add [{}] api_key in ~/.codewhale/config.toml.",
        provider.display_name(),
        credential_hint,
        provider.as_str(),
        provider.env_vars_label(),
        provider_config_table_name(provider)?
    ))
}

/// Clear every saved API key from config-file storage AND the durable
/// secret store.
///
/// The full-wipe logout path (`codewhale-tui --logout`, `auth logout`)
/// calls this to remove credentials so the next request can't
/// silently use a stale config key (#343). The function removes the legacy
/// root `api_key` entry *and* every `api_key` entry nested in a
/// `[providers.<name>]` table, leaving keys like `api_key_env`, comments,
/// and formatting untouched, then deletes every provider's secret-store
/// slot — symmetric with CLI logout (#5159) — so a stored credential cannot
/// survive logout and reappear through the read chain (#5196). The TUI
/// `/logout` command stays single-provider and goes through
/// [`clear_active_provider_api_key`] instead.
///
/// Environment variables (`DEEPSEEK_API_KEY`, etc.) are intentionally
/// **not** unset — they are managed by the user's shell and outside the
/// CLI's purview. `Config::deepseek_api_key`'s explicit-override path
/// (Path 0) ensures a freshly-entered key still wins over a stale env
/// var that lingers from a previous session.
pub fn clear_api_key() -> Result<()> {
    codewhale_config::with_xai_oauth_revocation_transaction(clear_api_key_unlocked)
}

fn clear_api_key_unlocked() -> Result<()> {
    // Same read-modify-write as the saves: hold every durable slot's write
    // lock across the config-document mutation and the store deletes so a
    // concurrent save cannot interleave and leave the two disagreeing.
    crate::credentials::store::with_provider_write_locks(
        known_secret_store_slots(),
        clear_api_key_under_slot_locks,
    )
}

fn clear_api_key_under_slot_locks() -> Result<()> {
    // Strip api_key entries from config.toml, including provider-scoped
    // nested entries. Clearing a config file must not trigger platform
    // credential prompts. Clears target the same user-global document that
    // credential saves write, so logout removes what login stored (#5045).
    let config_path = credential_config_path()
        .context("Failed to resolve config path while clearing API keys.")?;

    if config_path.exists() {
        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            crate::config_persistence::remove_document_key_recursive(doc.as_table_mut(), "api_key");
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "oauth_credential_generation"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "auth_mode"],
            )?;
            crate::config_persistence::unset_document_value(
                doc,
                &["providers", "xai", "external_credentials"],
            )?;
            Ok(())
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        log_sensitive_event(
            "credential.clear",
            json!({
                "backend": "config_file",
                "config_path": config_path.display().to_string(),
                "scope": "root_and_provider_keys",
            }),
        );
    }

    // The config scrub alone leaves the durable secret-store credential
    // alive, and the read chain prefers the secret store over the file, so a
    // "cleared" key silently came back on the next launch (#5196). Delete
    // every provider slot too, symmetric with CLI logout (#5159). This runs
    // even when the config file is absent: the slot survives independently
    // of the file.
    if let Some(secrets) = credential_secret_store() {
        let failures = clear_all_provider_api_keys_from_secret_store(secrets);
        if !failures.is_empty() {
            anyhow::bail!(
                "failed to delete stored credentials for: {}",
                failures.join(", ")
            );
        }
    }

    Ok(())
}

/// Delete the credential slot of every provider that has one stored.
///
/// Mirrors the CLI logout helper (#5159): each slot is probed first so
/// backends that error on deleting a missing item stay quiet, slots shared
/// by several providers (e.g. the historical `siliconflow` slot) are deleted
/// once, and every deletion failure is returned as a human-readable entry so
/// the caller can fail loudly instead of claiming a clean logout while
/// credentials linger in the store (#5196).
fn clear_all_provider_api_keys_from_secret_store(
    secrets: codewhale_secrets::Secrets,
) -> Vec<String> {
    let mut failures = Vec::new();
    let store = crate::credentials::store::SecretStoreCredentials::new(
        secrets.clone(),
        known_secret_store_slots(),
    );
    // `list` enumerates the slots that actually hold something, without
    // exposing any value — the deduplication that used to live here is now the
    // slot table's job.
    let stored: Vec<crate::credentials::CredentialInfo> = match store.list() {
        Ok(stored) => stored,
        Err(error) => {
            failures.push(format!("secret store enumeration: {error}"));
            return failures;
        }
    };
    for entry in stored {
        // The caller already holds this slot's write lock for the whole
        // logout. Delete through the backend rather than `store.delete`,
        // which would re-acquire the same non-reentrant mutex and deadlock.
        if let Err(error) = secrets.delete(&entry.provider_id) {
            failures.push(format!("{}: {error}", entry.provider_id));
        }
    }
    failures
}

/// Clear only the active provider's API key from the config file and delete
/// that provider's durable secret-store slot (#5196).
/// Unlike `clear_api_key()` which strips ALL api_key entries, this
/// removes only the key for the specified provider section (plus the
/// legacy root `api_key` when the provider is DeepSeek).
pub fn clear_active_provider_api_key(provider: &str) -> Result<()> {
    if provider == ApiProvider::Xai.as_str() {
        return codewhale_config::with_xai_oauth_revocation_transaction(|| {
            clear_active_provider_api_key_unlocked(provider)
        });
    }
    clear_active_provider_api_key_unlocked(provider)
}

fn clear_active_provider_api_key_unlocked(provider: &str) -> Result<()> {
    let slot = ApiProvider::all()
        .iter()
        .find(|candidate| candidate.as_str() == provider)
        .map(|candidate| provider_secret_store_slot(*candidate));
    match slot {
        Some(slot) => crate::credentials::store::with_provider_write_lock(slot, || {
            clear_active_provider_api_key_under_lock(provider)
        }),
        None => clear_active_provider_api_key_under_lock(provider),
    }
}

fn clear_active_provider_api_key_under_lock(provider: &str) -> Result<()> {
    let config_path = credential_config_path()
        .context("Failed to resolve config path while clearing API keys.")?;

    if config_path.exists() {
        // `custom` is both the legacy root-shaped route id and a valid exact
        // `[providers.custom]` table key. Inspect the persisted shape before the
        // mutation so logout clears exactly one credential scope.
        let persisted = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {}", config_path.display()))?;
        let persisted_config: Config = toml::from_str(&persisted).map_err(|_| {
            anyhow::anyhow!(
                "Failed to parse config from {}; file contents were omitted",
                codewhale_config::quote_os_path(&config_path)
            )
        })?;
        let exact_literal_custom_table = provider == ApiProvider::Custom.as_str()
            && persisted_config
                .providers
                .as_ref()
                .and_then(|providers| providers.custom_provider_config(provider))
                .is_some();

        crate::config_persistence::mutate_config_document(&config_path, |doc| {
            // The root-level api_key is shared by the legacy DeepSeek and released
            // literal-custom config shapes. Exact named custom ids remain scoped
            // to their own table.
            if matches!(
                provider,
                value if value == ApiProvider::Deepseek.as_str()
                    || value == ApiProvider::DeepseekCN.as_str()
            ) || (provider == ApiProvider::Custom.as_str() && !exact_literal_custom_table)
            {
                crate::config_persistence::unset_document_value(doc, &["api_key"])?;
            }
            if provider != ApiProvider::Custom.as_str() || exact_literal_custom_table {
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", provider, "api_key"],
                )?;
            }
            if provider == ApiProvider::Xai.as_str() {
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "oauth_credential_generation"],
                )?;
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "auth_mode"],
                )?;
                crate::config_persistence::unset_document_value(
                    doc,
                    &["providers", "xai", "external_credentials"],
                )?;
            }
            Ok(())
        })
        .with_context(|| format!("Failed to write config to {}", config_path.display()))?;
        log_sensitive_event(
            "credential.clear",
            json!({
                "backend": "config_file",
                "config_path": config_path.display().to_string(),
                "scope": provider,
            }),
        );
    }

    // The durable secret-store slot survives a config-file scrub and the
    // read chain prefers it, so the cleared key would silently come back
    // (#5196). Delete the provider's slot too — even when the config file
    // itself is absent. Exact named custom providers have no secret-store
    // slot, so an unmatched provider string skips this step.
    if let Some(secrets) = credential_secret_store()
        && let Some(slot) = ApiProvider::all()
            .iter()
            .find(|candidate| candidate.as_str() == provider)
            .map(|candidate| provider_secret_store_slot(*candidate))
    {
        let has_value = secrets
            .get(slot)
            .ok()
            .flatten()
            .is_some_and(|value| !value.trim().is_empty());
        if has_value {
            secrets
                .delete(slot)
                .with_context(|| format!("failed to delete stored credential for {slot}"))?;
        }
    }

    Ok(())
}
