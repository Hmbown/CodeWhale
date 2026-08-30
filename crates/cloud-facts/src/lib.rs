//! Cloud facts client: fetch `https://codewhale.net/api/facts/v1/<channel>`,
//! verify the Ed25519 envelope against the keys pinned in
//! `codewhale_config::cloud_facts::keys`, cache it under
//! `$CODEWHALE_HOME/facts/cloud-facts.json`, and install the scoped view as the
//! process-wide overlay. Modeled on the TUI's `models_dev_live` producer.
//!
//! Guarantees:
//! - Never a startup dependency: [`maybe_load_persisted_cache`] is a bounded
//!   synchronous disk read; all network happens in [`spawn_background_refresh`].
//! - Off by default (`[cloud_facts].enabled = false`); `CODEWHALE_CLOUD_FACTS=1`
//!   flips it, `CODEWHALE_DISABLE_CLOUD_FACTS=1` beats everything, CI markers
//!   suppress the fetch.
//! - The disk cache is re-verified on every load; a tampered file is deleted.
//! - The fetch sends only a fixed user agent and `If-None-Match`; no
//!   identifiers, cookies, or query parameters (PRD §5).
//! - With no active pinned key the layer is inert even when enabled.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use codewhale_config::catalog::now_unix;
use codewhale_config::cloud_facts::{
    CloudFactsState, CloudFactsStatus, FactsOrigin, FactsRejection, ScopedFacts, TrustedKey,
    VerifiedFacts, has_active_trusted_key, overlay, scoped_view, verify_envelope,
};
use codewhale_config::persistence::atomic_write;
use serde::{Deserialize, Serialize};

/// `{channel}` is replaced with the channel slug.
pub const DEFAULT_URL_TEMPLATE: &str = "https://codewhale.net/api/facts/v1/{channel}";
/// Refresh interval for a verified payload (6 h).
pub const DEFAULT_TTL_SECS: u64 = 6 * 60 * 60;
/// Bounded HTTP budget.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Largest response body accepted.
pub const MAX_BODY_BYTES: usize = codewhale_config::cloud_facts::MAX_ENVELOPE_BYTES;
/// Fixed, identifier-free user agent.
pub const USER_AGENT: &str = concat!("CodeWhale/", env!("CARGO_PKG_VERSION"), " (+cloud-facts)");
/// State subdir + file under `$CODEWHALE_HOME`.
pub const STATE_SUBDIR: &str = "facts";
pub const CACHE_FILE: &str = "cloud-facts.json";
const CACHE_SCHEMA_VERSION: u32 = 1;
const BACKOFF_BASE_SECS: u64 = 10 * 60;

/// Env: `1`/`0` overrides `[cloud_facts].enabled`.
pub const ENV_ENABLED: &str = "CODEWHALE_CLOUD_FACTS";
/// Env: hard kill switch (truthy) — beats config and `ENV_ENABLED`.
pub const ENV_DISABLE: &str = "CODEWHALE_DISABLE_CLOUD_FACTS";
/// Env: full URL override (may contain `{channel}`).
pub const ENV_URL: &str = "CODEWHALE_CLOUD_FACTS_URL";
/// Env: channel slug override.
pub const ENV_CHANNEL: &str = "CODEWHALE_CLOUD_FACTS_CHANNEL";
/// Env: read the envelope from a local file instead of the network.
pub const ENV_PATH: &str = "CODEWHALE_CLOUD_FACTS_PATH";
const CI_MARKERS: &[&str] = &[
    "CI",
    "GITHUB_ACTIONS",
    "GITLAB_CI",
    "BUILDKITE",
    "CIRCLECI",
    "JENKINS_URL",
    "TEAMCITY_VERSION",
    "TF_BUILD",
];

/// Resolved runtime settings (config + env).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub channel: String,
    pub url: Option<String>,
    pub ttl_secs: u64,
    /// Explicit cache file (tests); otherwise `$CODEWHALE_HOME/facts/cloud-facts.json`.
    pub cache_path: Option<PathBuf>,
    /// Local envelope path (`ENV_PATH`); skips the network.
    pub local_path: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: "stable".to_string(),
            url: None,
            ttl_secs: DEFAULT_TTL_SECS,
            cache_path: None,
            local_path: None,
        }
    }
}

fn env_truthy(name: &str) -> Option<bool> {
    let value = std::env::var(name).ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

impl Settings {
    /// Apply env overrides on top of config-derived settings.
    #[must_use]
    pub fn resolve(mut self) -> Self {
        if let Some(enabled) = env_truthy(ENV_ENABLED) {
            self.enabled = enabled;
        }
        if let Ok(channel) = std::env::var(ENV_CHANNEL) {
            let channel = channel.trim();
            if valid_channel(channel) {
                self.channel = channel.to_string();
            }
        }
        if let Ok(url) = std::env::var(ENV_URL) {
            let url = url.trim();
            if !url.is_empty() {
                self.url = Some(url.to_string());
            }
        }
        if let Ok(path) = std::env::var(ENV_PATH) {
            let path = path.trim();
            if !path.is_empty() {
                self.local_path = Some(PathBuf::from(path));
            }
        }
        self.ttl_secs = self.ttl_secs.max(60);
        self
    }

    /// The effective envelope URL.
    #[must_use]
    pub fn url(&self) -> String {
        self.url
            .as_deref()
            .unwrap_or(DEFAULT_URL_TEMPLATE)
            .replace("{channel}", &self.channel)
    }

    fn cache_file(&self) -> Option<PathBuf> {
        self.cache_path.clone().or_else(cache_path)
    }
}

/// Channel slugs are `[a-z0-9][a-z0-9-]{0,31}`.
#[must_use]
pub fn valid_channel(slug: &str) -> bool {
    let bytes = slug.as_bytes();
    (1..=32).contains(&bytes.len())
        && bytes[0].is_ascii_lowercase() | bytes[0].is_ascii_digit()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

/// Hard kill switch or CI: never fetch (a local `ENV_PATH` is still honoured).
#[must_use]
pub fn fetch_suppressed() -> bool {
    if env_truthy(ENV_DISABLE) == Some(true) {
        return true;
    }
    CI_MARKERS.iter().any(|name| {
        env_truthy(name).unwrap_or_else(|| std::env::var(name).is_ok_and(|v| !v.trim().is_empty()))
    })
}

/// Default cache path under the CodeWhale state root.
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    codewhale_config::resolve_state_dir(STATE_SUBDIR)
        .ok()
        .map(|dir| dir.join(CACHE_FILE))
}

/// On-disk cache. The envelope is stored verbatim and re-verified on load.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedCache {
    schema_version: u32,
    channel: String,
    url: String,
    fetched_at: u64,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    highest_seen_version: Option<u64>,
    #[serde(default)]
    backoff_until: Option<u64>,
    #[serde(default)]
    failures: u32,
    /// Raw envelope document (secret-free by construction); empty when the
    /// last fetch found no facts.
    #[serde(default)]
    envelope: String,
}

fn load_cache(path: &Path) -> Option<PersistedCache> {
    let bytes = std::fs::read(path).ok()?;
    let cache: PersistedCache = serde_json::from_slice(&bytes).ok()?;
    (cache.schema_version == CACHE_SCHEMA_VERSION).then_some(cache)
}

fn save_cache(path: &Path, cache: &PersistedCache) {
    match serde_json::to_vec(cache) {
        Ok(bytes) => {
            if let Err(err) = atomic_write(path, &bytes) {
                tracing::debug!(target: "cloud_facts", error = %err, "cache write failed");
            }
        }
        Err(err) => tracing::debug!(target: "cloud_facts", error = %err, "cache encode failed"),
    }
}

/// Why a refresh did not install new facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshError {
    Disabled,
    Inert,
    Suppressed,
    BackingOff { until: u64 },
    Network(String),
    HttpStatus(u16),
    TooLarge(usize),
    Rejected(FactsRejection),
    Io(String),
}

impl std::fmt::Display for RefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => write!(f, "cloud facts disabled"),
            Self::Inert => write!(f, "no active trusted key"),
            Self::Suppressed => write!(f, "fetch suppressed (kill switch or CI)"),
            Self::BackingOff { until } => write!(f, "backing off until {until}"),
            Self::Network(msg) => write!(f, "network: {msg}"),
            Self::HttpStatus(code) => write!(f, "HTTP {code}"),
            Self::TooLarge(bytes) => write!(f, "response too large ({bytes} bytes)"),
            Self::Rejected(reason) => write!(f, "{reason}"),
            Self::Io(msg) => write!(f, "io: {msg}"),
        }
    }
}

/// What a successful refresh did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// Server returned 304; the verified facts on disk are current.
    NotModified { facts_version: Option<u64> },
    /// A new envelope verified and is now the overlay.
    Updated { facts_version: u64 },
    /// Within TTL; nothing fetched.
    Fresh { facts_version: Option<u64> },
    /// Server has no facts for the channel (404); bundled in use.
    NoFacts,
}

fn set_state(state: CloudFactsState, etag: Option<String>, source_label: &str) {
    overlay::set_status(CloudFactsStatus {
        state,
        last_attempt: Some(now_unix()),
        etag,
        source_label: source_label.to_string(),
    });
}

fn current_verified_version() -> Option<u64> {
    match overlay::status().state {
        CloudFactsState::Verified { facts_version, .. } => Some(facts_version),
        _ => None,
    }
}

/// Install a verified payload as the overlay and report it.
fn install(
    verified: &VerifiedFacts,
    origin: FactsOrigin,
    fetched_at: u64,
    etag: Option<String>,
    source_label: &str,
) -> ScopedFacts {
    let scoped = scoped_view(
        verified,
        &codewhale_config::cloud_facts::current_version(),
        now_unix(),
    );
    let (patches, defaults, announcements) = scoped.item_counts();
    for receipt in &scoped.dropped {
        tracing::debug!(target: "cloud_facts", receipt, "cloud facts item dropped");
    }
    overlay::set_overlay(Some(scoped.clone()));
    set_state(
        CloudFactsState::Verified {
            channel: scoped.channel.clone(),
            facts_version: scoped.facts_version,
            key_id: scoped.key_id.clone(),
            fetched_at,
            origin,
            stale: scoped.stale,
            patches,
            defaults,
            announcements,
        },
        etag,
        source_label,
    );
    scoped
}

fn verify_with(
    bytes: &[u8],
    settings: &Settings,
    highest_seen: Option<u64>,
    keys: &[TrustedKey],
) -> Result<VerifiedFacts, FactsRejection> {
    verify_envelope(
        bytes,
        &settings.channel,
        &codewhale_config::cloud_facts::current_version(),
        highest_seen,
        keys,
        now_unix(),
    )
}

/// Seed the overlay from the disk cache before any picker/status read.
///
/// Returns the installed `facts_version`, if any. Bounded: one file read, one
/// verification. Tampered or unverifiable caches are deleted.
pub fn maybe_load_persisted_cache(settings: &Settings) -> Option<u64> {
    maybe_load_persisted_cache_with_keys(settings, codewhale_config::cloud_facts::TRUSTED_KEYS)
}

fn maybe_load_persisted_cache_with_keys(settings: &Settings, keys: &[TrustedKey]) -> Option<u64> {
    if !settings.enabled {
        set_state(CloudFactsState::Off, None, "");
        return None;
    }
    if !keys
        .iter()
        .any(|k| k.status == codewhale_config::cloud_facts::KeyStatus::Active)
    {
        set_state(CloudFactsState::Inert, None, "");
        return None;
    }
    set_state(CloudFactsState::BundledOnly, None, "");
    let path = settings.cache_file()?;
    let cache = load_cache(&path)?;
    if cache.channel != settings.channel || cache.envelope.trim().is_empty() {
        return None;
    }
    match verify_with(
        cache.envelope.as_bytes(),
        settings,
        cache.highest_seen_version,
        keys,
    ) {
        Ok(verified) => {
            let mut verified = verified;
            // Past TTL the payload is still installed, but flagged stale.
            if now_unix().saturating_sub(cache.fetched_at) > settings.ttl_secs {
                verified.stale = true;
            }
            let scoped = install(
                &verified,
                FactsOrigin::DiskCache,
                cache.fetched_at,
                cache.etag.clone(),
                &cache.url,
            );
            Some(scoped.facts_version)
        }
        Err(FactsRejection::NotApplicable { applies_to }) => {
            set_state(
                CloudFactsState::NotApplicable { applies_to },
                cache.etag,
                &cache.url,
            );
            None
        }
        Err(reason) => {
            tracing::debug!(target: "cloud_facts", error = %reason, "persisted cloud facts rejected; deleting cache");
            let _ = std::fs::remove_file(&path);
            set_state(
                CloudFactsState::Rejected {
                    reason: reason.to_string(),
                    at: now_unix(),
                },
                None,
                &cache.url,
            );
            None
        }
    }
}

enum Fetched {
    NotModified,
    NotFound,
    Body {
        bytes: Vec<u8>,
        etag: Option<String>,
    },
}

async fn fetch(url: &str, etag: Option<&str>) -> Result<Fetched, RefreshError> {
    let client = codewhale_release::tls::reqwest_client_builder()
        .timeout(FETCH_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|err| RefreshError::Network(err.to_string()))?;
    let mut request = client.get(url).header("Accept", "application/json");
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let response = request
        .send()
        .await
        .map_err(|err| RefreshError::Network(err.to_string()))?;
    let status = response.status().as_u16();
    if status == 304 {
        return Ok(Fetched::NotModified);
    }
    if status == 404 {
        return Ok(Fetched::NotFound);
    }
    if !(200..300).contains(&status) {
        return Err(RefreshError::HttpStatus(status));
    }
    if let Some(len) = response.content_length()
        && len as usize > MAX_BODY_BYTES
    {
        return Err(RefreshError::TooLarge(len as usize));
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .await
        .map_err(|err| RefreshError::Network(err.to_string()))?;
    if bytes.len() > MAX_BODY_BYTES {
        return Err(RefreshError::TooLarge(bytes.len()));
    }
    Ok(Fetched::Body {
        bytes: bytes.to_vec(),
        etag,
    })
}

fn record_failure(cache: &mut PersistedCache, err: &RefreshError, ttl_secs: u64) {
    cache.failures = cache.failures.saturating_add(1);
    let exp = cache.failures.min(10);
    let delay = (BACKOFF_BASE_SECS << (exp - 1)).min(ttl_secs);
    cache.backoff_until = Some(now_unix() + delay);
    set_state(
        CloudFactsState::Failed {
            last_error: err.to_string(),
            at: now_unix(),
            keeping: current_verified_version(),
        },
        cache.etag.clone(),
        &cache.url,
    );
}

/// Refresh from the local path or the network and install the result.
///
/// `force` bypasses the TTL and backoff (manual `/facts refresh`).
pub async fn refresh(settings: &Settings, force: bool) -> Result<RefreshOutcome, RefreshError> {
    refresh_with_keys(settings, force, codewhale_config::cloud_facts::TRUSTED_KEYS).await
}

async fn refresh_with_keys(
    settings: &Settings,
    force: bool,
    keys: &[TrustedKey],
) -> Result<RefreshOutcome, RefreshError> {
    if !settings.enabled {
        set_state(CloudFactsState::Off, None, "");
        return Err(RefreshError::Disabled);
    }
    if !keys
        .iter()
        .any(|k| k.status == codewhale_config::cloud_facts::KeyStatus::Active)
    {
        set_state(CloudFactsState::Inert, None, "");
        return Err(RefreshError::Inert);
    }
    let path = settings.cache_file();
    let mut cache = path
        .as_deref()
        .and_then(load_cache)
        .filter(|c| c.channel == settings.channel)
        .unwrap_or_else(|| PersistedCache {
            schema_version: CACHE_SCHEMA_VERSION,
            channel: settings.channel.clone(),
            ..PersistedCache::default()
        });

    let now = now_unix();
    let (bytes, source_label, etag) = if let Some(local) = &settings.local_path {
        let bytes = std::fs::read(local).map_err(|err| {
            let mapped = RefreshError::Io(err.to_string());
            set_state(
                CloudFactsState::Failed {
                    last_error: mapped.to_string(),
                    at: now,
                    keeping: current_verified_version(),
                },
                None,
                &local.display().to_string(),
            );
            mapped
        })?;
        (bytes, format!("file:{}", local.display()), None)
    } else {
        if fetch_suppressed() {
            return Err(RefreshError::Suppressed);
        }
        if !force {
            if let Some(until) = cache.backoff_until
                && now < until
            {
                return Err(RefreshError::BackingOff { until });
            }
            if !cache.envelope.is_empty()
                && now.saturating_sub(cache.fetched_at) < settings.ttl_secs
                && current_verified_version().is_some()
            {
                return Ok(RefreshOutcome::Fresh {
                    facts_version: current_verified_version(),
                });
            }
        }
        let url = settings.url();
        cache.url = url.clone();
        let etag_hint = if cache.envelope.is_empty() {
            None
        } else {
            cache.etag.as_deref()
        };
        match fetch(&url, etag_hint).await {
            Ok(Fetched::NotModified) => {
                cache.fetched_at = now;
                cache.failures = 0;
                cache.backoff_until = None;
                if let Some(path) = &path {
                    save_cache(path, &cache);
                }
                // Keep the verified overlay; refresh the timestamp/origin.
                if let CloudFactsState::Verified {
                    channel,
                    facts_version,
                    key_id,
                    patches,
                    defaults,
                    announcements,
                    ..
                } = overlay::status().state
                {
                    set_state(
                        CloudFactsState::Verified {
                            channel,
                            facts_version,
                            key_id,
                            fetched_at: now,
                            origin: FactsOrigin::Network,
                            stale: false,
                            patches,
                            defaults,
                            announcements,
                        },
                        cache.etag.clone(),
                        &url,
                    );
                }
                return Ok(RefreshOutcome::NotModified {
                    facts_version: current_verified_version(),
                });
            }
            Ok(Fetched::NotFound) => {
                cache.fetched_at = now;
                cache.failures = 0;
                cache.backoff_until = None;
                cache.envelope.clear();
                cache.etag = None;
                if let Some(path) = &path {
                    save_cache(path, &cache);
                }
                overlay::set_overlay(None);
                set_state(CloudFactsState::BundledOnly, None, &url);
                return Ok(RefreshOutcome::NoFacts);
            }
            Ok(Fetched::Body { bytes, etag }) => (bytes, url, etag),
            Err(err) => {
                record_failure(&mut cache, &err, settings.ttl_secs);
                if let Some(path) = &path {
                    save_cache(path, &cache);
                }
                return Err(err);
            }
        }
    };

    match verify_with(&bytes, settings, cache.highest_seen_version, keys) {
        Ok(verified) => {
            let origin = if settings.local_path.is_some() {
                FactsOrigin::LocalFile
            } else {
                FactsOrigin::Network
            };
            let scoped = install(&verified, origin, now, etag.clone(), &source_label);
            cache.fetched_at = now;
            cache.etag = etag;
            cache.failures = 0;
            cache.backoff_until = None;
            cache.highest_seen_version = Some(
                cache
                    .highest_seen_version
                    .map_or(scoped.facts_version, |h| h.max(scoped.facts_version)),
            );
            cache.envelope = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(path) = &path {
                save_cache(path, &cache);
            }
            Ok(RefreshOutcome::Updated {
                facts_version: scoped.facts_version,
            })
        }
        Err(FactsRejection::NotApplicable { applies_to }) => {
            // Verified, just not for this build. Cache it so the ETag saves a
            // round trip; the overlay stays whatever it was.
            cache.fetched_at = now;
            cache.etag = etag.clone();
            cache.failures = 0;
            cache.backoff_until = None;
            cache.envelope = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(path) = &path {
                save_cache(path, &cache);
            }
            set_state(
                CloudFactsState::NotApplicable {
                    applies_to: applies_to.clone(),
                },
                etag,
                &source_label,
            );
            Err(RefreshError::Rejected(FactsRejection::NotApplicable {
                applies_to,
            }))
        }
        Err(reason) => {
            let err = RefreshError::Rejected(reason.clone());
            record_failure(&mut cache, &err, settings.ttl_secs);
            if current_verified_version().is_none() {
                set_state(
                    CloudFactsState::Rejected {
                        reason: reason.to_string(),
                        at: now,
                    },
                    cache.etag.clone(),
                    &source_label,
                );
            }
            if let Some(path) = &path {
                save_cache(path, &cache);
            }
            Err(err)
        }
    }
}

/// Best-effort background refresh; never panics, never blocks the caller.
///
/// `on_update` runs after a new payload is installed (the TUI uses it to
/// invalidate its memoized catalog merge).
pub fn spawn_background_refresh(
    settings: Settings,
    on_update: Option<Arc<dyn Fn() + Send + Sync>>,
) {
    if !settings.enabled || !has_active_trusted_key() {
        return;
    }
    if settings.local_path.is_none() && fetch_suppressed() {
        return;
    }
    tokio::spawn(async move {
        match refresh(&settings, false).await {
            Ok(outcome) => {
                tracing::debug!(target: "cloud_facts", ?outcome, "cloud facts refreshed");
                if matches!(
                    outcome,
                    RefreshOutcome::Updated { .. } | RefreshOutcome::NoFacts
                ) && let Some(hook) = on_update
                {
                    hook();
                }
            }
            Err(err) => {
                tracing::debug!(target: "cloud_facts", error = %err, "cloud facts refresh skipped");
            }
        }
    });
}

/// Current status (re-exported for callers that only depend on this crate).
#[must_use]
pub fn status() -> CloudFactsStatus {
    overlay::status()
}

#[cfg(test)]
mod tests;
