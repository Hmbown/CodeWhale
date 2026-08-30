//! Durable, secret-free per-provider `/models` catalog cache.
//!
//! This is the persistence owner for [`codewhale_config::catalog::ProviderCatalogCache`].
//! It replaces the previous process-only provider refresh path: successful
//! refreshes replace one exact `(provider identity, base URL fingerprint)`
//! partition, failures retain that partition's prior rows, and startup loads
//! only the active route's exact partition. Credentials authorize the fetch in
//! `client`; they never enter this module or its disk envelope.
//!
//! This deliberately does not import the legacy `model_catalog` cache at
//! `catalog/openrouter.json`: that file has no provider/base-URL scope, so
//! treating it as a provider-owned roster could leak stale facts across custom
//! endpoints. `model_catalog` remains a read-only compatibility fallback for
//! older model-metadata consumers while provider-lake/runtime consumers migrate;
//! `catalog/provider-catalogs.json` is the sole writer-owned live roster store.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use anyhow::{Context, Result};
use codewhale_config::catalog::now_unix;
use codewhale_config::catalog::{
    CatalogRefreshError, CatalogSnapshot, CatalogStatus, ProviderCatalogCache,
    ProviderCatalogDelta, base_url_fingerprint,
};
use codewhale_config::persistence::atomic_write_json;
use codewhale_config::pricing::{Currency, OfferingPricing, PricingProvenance};
use serde::{Deserialize, Serialize};

use crate::config::{ApiProvider, Config};

const CACHE_SCHEMA_VERSION: u32 = 1;
const CACHE_FILE: &str = "provider-catalogs.json";
const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;
const MAX_CACHE_SCOPES: usize = 64;
const MAX_CACHE_ROWS: usize = 50_000;

#[derive(Debug, Clone, Copy)]
struct CachePersistenceLimits {
    max_bytes: u64,
    max_scopes: usize,
    max_rows: usize,
}

const CACHE_PERSISTENCE_LIMITS: CachePersistenceLimits = CachePersistenceLimits {
    max_bytes: MAX_CACHE_BYTES,
    max_scopes: MAX_CACHE_SCOPES,
    max_rows: MAX_CACHE_ROWS,
};

/// Provider-owned catalogs are refreshed daily. Past-TTL rows remain visible
/// with an explicit stale receipt until a successful replacement arrives.
pub const DEFAULT_PROVIDER_CATALOG_TTL_SECS: u64 = 24 * 60 * 60;

static CACHE: LazyLock<RwLock<ProviderCatalogCache>> =
    LazyLock::new(|| RwLock::new(ProviderCatalogCache::new()));
static REFRESH_GENERATIONS: LazyLock<RwLock<BTreeMap<String, u64>>> =
    LazyLock::new(|| RwLock::new(BTreeMap::new()));

#[derive(Debug, Clone)]
pub struct ProviderCatalogRefreshTicket {
    provider: String,
    generation: u64,
}

/// Immutable, secret-free provider-live rate evidence captured at dispatch.
///
/// Rates are stored as canonical decimal strings rather than `f64` so route
/// receipts retain exact equality and stable JSON. `catalog_revision` binds
/// every identity, scope, timestamp, currency, provenance, and rate field; it
/// therefore changes even when two refreshes land in the same Unix second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderLivePricingQuote {
    pub(crate) provider: ApiProvider,
    pub(crate) provider_identity: String,
    pub(crate) wire_model: String,
    pub(crate) endpoint_fingerprint: String,
    pub(crate) catalog_fetched_at: u64,
    pub(crate) catalog_revision: String,
    pub(crate) currency: Currency,
    pub(crate) provenance: PricingProvenance,
    pub(crate) input_per_million: Option<String>,
    pub(crate) output_per_million: Option<String>,
    pub(crate) cache_read_per_million: Option<String>,
    pub(crate) cache_write_per_million: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct ProviderLivePricingQuoteWire {
    provider: ApiProvider,
    provider_identity: String,
    wire_model: String,
    endpoint_fingerprint: String,
    catalog_fetched_at: u64,
    catalog_revision: String,
    currency: Currency,
    provenance: PricingProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_per_million: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output_per_million: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read_per_million: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write_per_million: Option<String>,
}

impl From<&ProviderLivePricingQuote> for ProviderLivePricingQuoteWire {
    fn from(quote: &ProviderLivePricingQuote) -> Self {
        Self {
            provider: quote.provider,
            provider_identity: quote.provider_identity.clone(),
            wire_model: quote.wire_model.clone(),
            endpoint_fingerprint: quote.endpoint_fingerprint.clone(),
            catalog_fetched_at: quote.catalog_fetched_at,
            catalog_revision: quote.catalog_revision.clone(),
            currency: quote.currency.clone(),
            provenance: quote.provenance.clone(),
            input_per_million: quote.input_per_million.clone(),
            output_per_million: quote.output_per_million.clone(),
            cache_read_per_million: quote.cache_read_per_million.clone(),
            cache_write_per_million: quote.cache_write_per_million.clone(),
        }
    }
}

impl Serialize for ProviderLivePricingQuote {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        if !self.is_structurally_valid() {
            return serializer.serialize_none();
        }
        ProviderLivePricingQuoteWire::from(self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ProviderLivePricingQuote {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = ProviderLivePricingQuoteWire::deserialize(deserializer)?;
        let quote = Self {
            provider: wire.provider,
            provider_identity: wire.provider_identity,
            wire_model: wire.wire_model,
            endpoint_fingerprint: wire.endpoint_fingerprint,
            catalog_fetched_at: wire.catalog_fetched_at,
            catalog_revision: wire.catalog_revision,
            currency: wire.currency,
            provenance: wire.provenance,
            input_per_million: wire.input_per_million,
            output_per_million: wire.output_per_million,
            cache_read_per_million: wire.cache_read_per_million,
            cache_write_per_million: wire.cache_write_per_million,
        };
        quote
            .is_structurally_valid()
            .then_some(quote)
            .ok_or_else(|| serde::de::Error::custom("invalid provider-live pricing quote"))
    }
}

pub(crate) fn deserialize_optional_provider_live_pricing<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<ProviderLivePricingQuote>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

impl ProviderLivePricingQuote {
    fn is_structurally_valid(&self) -> bool {
        self.pricing_for_route(
            self.provider,
            &self.provider_identity,
            &self.wire_model,
            &self.endpoint_fingerprint,
            self.catalog_fetched_at,
        )
        .is_some()
    }
    fn canonical_rate(rate: Option<f64>) -> Option<String> {
        rate.map(|rate| rate.to_string())
    }

    fn revision_for(
        provider: ApiProvider,
        provider_identity: &str,
        wire_model: &str,
        endpoint_fingerprint: &str,
        catalog_fetched_at: u64,
        currency: &Currency,
        provenance: &PricingProvenance,
        input_per_million: &Option<String>,
        output_per_million: &Option<String>,
        cache_read_per_million: &Option<String>,
        cache_write_per_million: &Option<String>,
    ) -> Option<String> {
        let payload = serde_json::to_vec(&(
            "codewhale-provider-live-pricing-quote-v1",
            provider,
            provider_identity,
            wire_model,
            endpoint_fingerprint,
            catalog_fetched_at,
            currency,
            provenance,
            input_per_million,
            output_per_million,
            cache_read_per_million,
            cache_write_per_million,
        ))
        .ok()?;
        Some(format!("sha256:{}", crate::hashing::sha256_hex(payload)))
    }

    fn from_pricing(
        provider: ApiProvider,
        provider_identity: &str,
        wire_model: &str,
        endpoint_fingerprint: &str,
        catalog_fetched_at: u64,
        pricing: &OfferingPricing,
    ) -> Option<Self> {
        let provider_identity = provider_identity.trim();
        let wire_model = wire_model.trim();
        if crate::cost_status::sanitize_persisted_route_label(provider_identity)
            != provider_identity
            || crate::cost_status::sanitize_persisted_route_label(wire_model) != wire_model
        {
            return None;
        }
        let input_per_million = Self::canonical_rate(pricing.input_per_million);
        let output_per_million = Self::canonical_rate(pricing.output_per_million);
        let cache_read_per_million = Self::canonical_rate(pricing.cache_read_per_million);
        let cache_write_per_million = Self::canonical_rate(pricing.cache_write_per_million);
        let catalog_revision = Self::revision_for(
            provider,
            provider_identity,
            wire_model,
            endpoint_fingerprint,
            catalog_fetched_at,
            &pricing.currency,
            &pricing.provenance,
            &input_per_million,
            &output_per_million,
            &cache_read_per_million,
            &cache_write_per_million,
        )?;
        Some(Self {
            provider,
            provider_identity: provider_identity.to_string(),
            wire_model: wire_model.to_string(),
            endpoint_fingerprint: endpoint_fingerprint.to_string(),
            catalog_fetched_at,
            catalog_revision,
            currency: pricing.currency.clone(),
            provenance: pricing.provenance.clone(),
            input_per_million,
            output_per_million,
            cache_read_per_million,
            cache_write_per_million,
        })
    }

    fn parse_rate(rate: &Option<String>) -> Option<Option<f64>> {
        let Some(rate) = rate else {
            return Some(None);
        };
        let parsed = rate.parse::<f64>().ok()?;
        (parsed.is_finite() && parsed >= 0.0 && parsed.to_string() == *rate).then_some(Some(parsed))
    }

    /// Rehydrate the frozen row only when every receipt binding is intact.
    /// This is deliberately cache-free: a refresh after dispatch cannot alter
    /// an earlier turn, while malformed or legacy receipts fail closed.
    pub(crate) fn pricing_for_route(
        &self,
        provider: ApiProvider,
        provider_identity: &str,
        wire_model: &str,
        endpoint_fingerprint: &str,
        dispatched_at_unix: u64,
    ) -> Option<OfferingPricing> {
        let provider_identity = provider_identity.trim();
        let wire_model = wire_model.trim();
        if self.provider != provider
            || crate::cost_status::sanitize_persisted_route_label(&self.provider_identity)
                != self.provider_identity
            || crate::cost_status::sanitize_persisted_route_label(&self.wire_model)
                != self.wire_model
            || self.endpoint_fingerprint.len() != 64
            || !self
                .endpoint_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || self.provider_identity != provider_identity
            || self.wire_model != wire_model
            || self.endpoint_fingerprint != endpoint_fingerprint
            || self.catalog_fetched_at > dispatched_at_unix
            || dispatched_at_unix.saturating_sub(self.catalog_fetched_at)
                >= DEFAULT_PROVIDER_CATALOG_TTL_SECS
            || self.currency != Currency::Usd
            || self.provenance != PricingProvenance::ProviderLive
            || !reviewed_provider_live_scope(provider, provider_identity, endpoint_fingerprint)
        {
            return None;
        }
        let input_per_million = Self::parse_rate(&self.input_per_million)?;
        let output_per_million = Self::parse_rate(&self.output_per_million)?;
        let cache_read_per_million = Self::parse_rate(&self.cache_read_per_million)?;
        let cache_write_per_million = Self::parse_rate(&self.cache_write_per_million)?;
        let cost = codewhale_config::models_dev::ModelsDevCost {
            input: input_per_million,
            output: output_per_million,
            cache_read: cache_read_per_million,
            cache_write: cache_write_per_million,
        };
        if !codewhale_config::pricing::catalog_cost_is_valid(&cost) {
            return None;
        }
        // A reviewed per-token route needs both ordinary request classes. Cache
        // classes remain optional and fail closed later if a turn used them.
        if cost.input.is_none() || cost.output.is_none() {
            return None;
        }
        let expected_revision = Self::revision_for(
            self.provider,
            &self.provider_identity,
            &self.wire_model,
            &self.endpoint_fingerprint,
            self.catalog_fetched_at,
            &self.currency,
            &self.provenance,
            &self.input_per_million,
            &self.output_per_million,
            &self.cache_read_per_million,
            &self.cache_write_per_million,
        )?;
        if self.catalog_revision != expected_revision {
            return None;
        }
        Some(OfferingPricing {
            provider: self.provider_identity.clone(),
            wire_model_id: self.wire_model.clone(),
            canonical_model: None,
            currency: self.currency.clone(),
            input_per_million: cost.input,
            output_per_million: cost.output,
            cache_read_per_million: cost.cache_read,
            cache_write_per_million: cost.cache_write,
            provenance: self.provenance.clone(),
            effective_at: Some(self.catalog_fetched_at),
            endpoint_fingerprint: Some(self.endpoint_fingerprint.clone()),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedProviderCatalogs {
    schema_version: u32,
    cache: ProviderCatalogCache,
}

#[derive(Serialize)]
struct PersistedProviderCatalogsRef<'a> {
    schema_version: u32,
    cache: &'a ProviderCatalogCache,
}

/// Resolve the cache under Codewhale's catalog state directory.
///
/// Unguarded tests are confined to the TUI test root, matching the Models.dev
/// cache contract, so they never inspect a developer's real provider catalog.
#[must_use]
pub fn cache_path() -> Option<PathBuf> {
    #[cfg(test)]
    {
        if !crate::test_support::guarded_environment_provides_state_paths() {
            return Some(
                crate::test_support::unsealed_test_state_root()
                    .join("catalog")
                    .join(CACHE_FILE),
            );
        }
    }
    codewhale_config::resolve_state_dir("catalog")
        .ok()
        .map(|dir| dir.join(CACHE_FILE))
}

fn canonical_provider_scope(provider: &str) -> String {
    // Despite the historical name, this is the exact configured ownership
    // scope. Never collapse a custom table that happens to resemble a built-in
    // or setup-template alias.
    crate::provider_lake::catalog_partition_key(provider)
}

fn is_account_scoped_provider(provider: &str) -> bool {
    codewhale_config::provider_setup_template(provider)
        .is_some_and(|template| template.id == codewhale_config::BASETEN_TEMPLATE_ID)
}

fn cache_lock_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| CACHE_FILE.into());
    name.push(".lock");
    path.with_file_name(name)
}

fn open_cache_lock(path: &Path) -> Result<fs::File> {
    let parent = path
        .parent()
        .context("provider catalog lock path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create provider catalog directory {}", parent.display()))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options
        .open(path)
        .with_context(|| format!("open provider catalog lock {}", path.display()))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect provider catalog lock {}", path.display()))?;
    anyhow::ensure!(
        metadata.is_file(),
        "provider catalog lock {} must be a regular file",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        anyhow::ensure!(
            metadata.nlink() == 1,
            "provider catalog lock {} must not be hard linked",
            path.display()
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        anyhow::ensure!(
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
            "provider catalog lock {} must not be a reparse point",
            path.display()
        );
    }
    Ok(file)
}

fn load_from_disk_unlocked_with_limit(path: &Path, max_bytes: u64) -> Option<ProviderCatalogCache> {
    let file = fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() > max_bytes {
        tracing::debug!(
            target: "provider_catalog",
            path = %path.display(),
            max_bytes,
            "provider catalog cache exceeds read limit"
        );
        return None;
    }
    // Re-check through `take`: the file can grow after metadata is sampled.
    let mut body = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut body)
        .ok()?;
    if body.len() as u64 > max_bytes {
        return None;
    }
    let persisted: PersistedProviderCatalogs = serde_json::from_slice(&body).ok()?;
    if persisted.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }
    let mut cache = persisted.cache;
    // Older builds could durably cache Baseten's account-scoped roster. Scrub
    // those entries on every load so upgrading cannot attach one workspace's
    // catalog to a different credential.
    cache
        .entries
        .retain(|_, entry| !is_account_scoped_provider(&entry.provider));
    Some(cache)
}

fn load_from_disk_unlocked(path: &Path) -> Option<ProviderCatalogCache> {
    load_from_disk_unlocked_with_limit(path, MAX_CACHE_BYTES)
}

fn load_from_disk() -> Option<ProviderCatalogCache> {
    let path = cache_path()?;
    if !path.is_file() {
        return None;
    }
    let lock_file = open_cache_lock(&cache_lock_path(&path)).ok()?;
    let lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock.read().ok()?;
    load_from_disk_unlocked(&path)
}

fn merge_durable_scope(
    mut durable_cache: ProviderCatalogCache,
    process_cache: &ProviderCatalogCache,
    provider: &str,
    fingerprint: &str,
) -> ProviderCatalogCache {
    durable_cache
        .entries
        .retain(|_, entry| !is_account_scoped_provider(&entry.provider));
    if !is_account_scoped_provider(provider)
        && let Some(entry) = process_cache.get(provider, fingerprint).cloned()
    {
        durable_cache.entries.insert(
            ProviderCatalogCache::cache_key(provider, fingerprint),
            entry,
        );
    }
    durable_cache
}

fn persisted_envelope_len(cache: &ProviderCatalogCache) -> Result<u64> {
    let envelope = PersistedProviderCatalogsRef {
        schema_version: CACHE_SCHEMA_VERSION,
        cache,
    };
    let mut body = serde_json::to_vec_pretty(&envelope)
        .context("serialize provider catalog cache for bounded persistence")?;
    body.push(b'\n');
    u64::try_from(body.len()).context("provider catalog cache length exceeds u64")
}

fn cached_row_count(cache: &ProviderCatalogCache) -> usize {
    cache.entries.values().fold(0usize, |total, entry| {
        total.saturating_add(entry.offerings.len())
    })
}

/// Compact a durable cache without ever truncating one provider roster.
///
/// The exact scope being written is protected: if that scope alone fits, older
/// failed/stale scopes are evicted whole until the envelope is bounded. If the
/// protected scope alone does not fit, persistence is refused and the prior
/// atomic file remains intact. This avoids both self-bricking the 32 MiB read
/// limit and turning a partial provider roster into false authoritative truth.
fn bounded_cache_for_persistence(
    mut cache: ProviderCatalogCache,
    protected_scope: Option<(&str, &str)>,
    now: u64,
    limits: CachePersistenceLimits,
) -> Result<ProviderCatalogCache> {
    cache
        .entries
        .retain(|_, entry| !is_account_scoped_provider(&entry.provider));

    let protected_key = protected_scope
        .filter(|(provider, _)| !is_account_scoped_provider(provider))
        .map(|(provider, fingerprint)| ProviderCatalogCache::cache_key(provider, fingerprint));

    if let Some(key) = protected_key.as_deref()
        && let Some(entry) = cache.entries.get(key).cloned()
    {
        let mut protected_only = ProviderCatalogCache::new();
        protected_only.entries.insert(key.to_string(), entry);
        anyhow::ensure!(
            protected_only.entries.len() <= limits.max_scopes.min(MAX_CACHE_SCOPES)
                && cached_row_count(&protected_only) <= limits.max_rows
                && persisted_envelope_len(&protected_only)? <= limits.max_bytes,
            "provider catalog scope {key:?} exceeds bounded persistence limits"
        );
    }

    // Rank once while the cache/file locks are held. An older implementation
    // reserialized and rescanned the entire envelope for every eviction, which
    // made a valid sub-32-MiB file with many tiny scopes quadratic to compact.
    let mut eviction_keys = cache
        .entries
        .iter()
        .filter(|(key, _)| protected_key.as_deref() != Some(key.as_str()))
        .map(|(key, entry)| {
            let health_rank = if matches!(entry.status, CatalogStatus::Failed { .. }) {
                0u8
            } else if entry.is_stale(now) || matches!(entry.status, CatalogStatus::Stale { .. }) {
                1u8
            } else {
                2u8
            };
            (health_rank, entry.fetched_at, key.clone())
        })
        .collect::<Vec<_>>();
    eviction_keys.sort();
    let eviction_keys = eviction_keys
        .into_iter()
        .map(|(_, _, key)| key)
        .collect::<Vec<_>>();
    let mut eviction_index = 0usize;
    let mut rows = cached_row_count(&cache);
    let max_scopes = limits.max_scopes.min(MAX_CACHE_SCOPES);

    let mut evict_next = |cache: &mut ProviderCatalogCache| -> Result<usize> {
        let key = eviction_keys
            .get(eviction_index)
            .context("provider catalog envelope cannot fit even after whole-scope compaction")?;
        eviction_index = eviction_index.saturating_add(1);
        let entry = cache
            .entries
            .remove(key)
            .context("provider catalog eviction candidate disappeared")?;
        Ok(entry.offerings.len())
    };

    // First enforce the cheap cardinality limits in bulk. Only after at most 64
    // scopes remain do we serialize to enforce the exact on-disk byte limit.
    while cache.entries.len() > max_scopes || rows > limits.max_rows {
        rows = rows.saturating_sub(evict_next(&mut cache)?);
    }
    while persisted_envelope_len(&cache)? > limits.max_bytes {
        let _removed_rows = evict_next(&mut cache)?;
    }

    Ok(cache)
}

fn write_bounded_cache(
    path: &Path,
    cache: ProviderCatalogCache,
    protected_scope: Option<(&str, &str)>,
    limits: CachePersistenceLimits,
) -> Result<()> {
    let cache = bounded_cache_for_persistence(cache, protected_scope, now_unix(), limits)?;
    let envelope = PersistedProviderCatalogs {
        schema_version: CACHE_SCHEMA_VERSION,
        cache,
    };
    anyhow::ensure!(
        persisted_envelope_len(&envelope.cache)? <= limits.max_bytes,
        "bounded provider catalog cache exceeds its write limit"
    );
    atomic_write_json(path, &envelope)
}

fn persist_scope(cache: &ProviderCatalogCache, provider: &str, fingerprint: &str) {
    let Some(path) = cache_path() else {
        return;
    };
    let provider = canonical_provider_scope(provider);
    let result = (|| -> Result<()> {
        let lock_file = open_cache_lock(&cache_lock_path(&path))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock
            .write()
            .with_context(|| format!("write-lock provider catalog cache {}", path.display()))?;
        // Merge only the exact scope this process just changed into the latest
        // disk snapshot. A stale long-running TUI therefore cannot erase a
        // different scope written by the Runtime API (or vice versa).
        let durable_cache = merge_durable_scope(
            load_from_disk_unlocked(&path).unwrap_or_default(),
            cache,
            &provider,
            fingerprint,
        );
        write_bounded_cache(
            &path,
            durable_cache,
            Some((&provider, fingerprint)),
            CACHE_PERSISTENCE_LIMITS,
        )
        .with_context(|| format!("atomically write provider catalog {}", path.display()))
    })();
    if let Err(error) = result {
        tracing::debug!(
            target: "provider_catalog",
            error = %error,
            "provider catalog cache write failed"
        );
    }
}

/// Persist a failure without letting a stale process replace newer rows from
/// another Codewhale process for the same exact scope.
///
/// The ordinary scoped merge is sufficient for successes because the response
/// being committed is the new roster. A failure is different: its process may
/// have started with an older last-known-good entry. Re-read the durable exact
/// scope while holding the cross-process write lock, prefer it when it is at
/// least as recent, then change only the status before writing. Thus a failed
/// refresh can preserve the newest roster without resurrecting its own stale
/// snapshot over another process's success.
fn persist_failure_scope(
    cache: &mut ProviderCatalogCache,
    provider: &str,
    fingerprint: &str,
    reason: CatalogRefreshError,
) {
    let Some(path) = cache_path() else {
        return;
    };
    let provider = canonical_provider_scope(provider);
    let result = (|| -> Result<()> {
        let lock_file = open_cache_lock(&cache_lock_path(&path))?;
        let mut lock = fd_lock::RwLock::new(lock_file);
        let _guard = lock
            .write()
            .with_context(|| format!("write-lock provider catalog cache {}", path.display()))?;
        let durable_cache = load_from_disk_unlocked(&path).unwrap_or_default();

        if !is_account_scoped_provider(&provider)
            && let Some(durable_entry) = durable_cache.get(&provider, fingerprint).cloned()
        {
            let durable_is_newer = cache
                .get(&provider, fingerprint)
                .is_none_or(|local| durable_entry.fetched_at >= local.fetched_at);
            if durable_is_newer {
                cache.entries.insert(
                    ProviderCatalogCache::cache_key(&provider, fingerprint),
                    durable_entry,
                );
                cache.record_failure(&provider, fingerprint, reason);
            }
        }

        let durable_cache = merge_durable_scope(durable_cache, cache, &provider, fingerprint);
        write_bounded_cache(
            &path,
            durable_cache,
            Some((&provider, fingerprint)),
            CACHE_PERSISTENCE_LIMITS,
        )
        .with_context(|| format!("atomically write provider catalog {}", path.display()))
    })();
    if let Err(error) = result {
        tracing::debug!(
            target: "provider_catalog",
            error = %error,
            "provider catalog failure receipt write failed"
        );
    }
}

fn publish_exact_scope(cache: &ProviderCatalogCache, provider: &str, fingerprint: &str) -> usize {
    let provider = canonical_provider_scope(provider);
    let provider_kind = if codewhale_config::provider_setup_template(&provider)
        .is_some_and(|template| template.is_compatible())
    {
        ApiProvider::Custom
    } else {
        ApiProvider::parse(&provider).unwrap_or(ApiProvider::Custom)
    };
    publish_exact_scope_for_identity(cache, provider_kind, &provider, fingerprint)
}

fn publish_exact_scope_for_identity(
    cache: &ProviderCatalogCache,
    provider_kind: ApiProvider,
    provider_identity: &str,
    fingerprint: &str,
) -> usize {
    let provider = canonical_provider_scope(provider_identity);
    let offerings = cache
        .get(&provider, fingerprint)
        .map(|entry| entry.offerings.clone())
        .unwrap_or_default();
    let count = offerings.len();
    crate::provider_lake::replace_provider_live_snapshot_for_identity(
        provider_kind,
        &provider,
        CatalogSnapshot { offerings },
    );
    count
}

/// Load and publish only the active route's exact provider/base-URL scope.
///
/// A cache created for another custom endpoint or for an old endpoint override
/// is retained on disk but cannot leak into the active picker.
pub fn maybe_load_persisted_cache_for_config(config: &Config) -> usize {
    let provider = config.api_provider();
    let provider_identity = canonical_provider_scope(&config.provider_identity_for(provider));
    let fingerprint = base_url_fingerprint(&config.deepseek_base_url());
    if is_account_scoped_provider(&provider_identity) {
        forget_account_scoped_provider(&provider_identity);
        return 0;
    }
    if let Ok(mut guard) = CACHE.write()
        && let Some(loaded) = load_from_disk()
    {
        // Keep session-only scopes that cannot exist on disk, while allowing a
        // newer durable scope from another Codewhale process to refresh this
        // process. Every in-process writer takes CACHE before the file lock, so
        // this read/merge cannot overwrite a concurrent local refresh.
        for (key, entry) in loaded.entries {
            let should_replace = guard
                .entries
                .get(&key)
                .is_none_or(|current| entry.fetched_at >= current.fetched_at);
            if should_replace {
                guard.entries.insert(key, entry);
            }
        }
    }
    CACHE
        .read()
        .map(|guard| {
            publish_exact_scope_for_identity(&guard, provider, &provider_identity, &fingerprint)
        })
        .unwrap_or(0)
}

fn forget_account_scoped_provider(provider: &str) {
    let provider = canonical_provider_scope(provider);
    if let Ok(mut cache) = CACHE.write() {
        cache
            .entries
            .retain(|_, entry| canonical_provider_scope(&entry.provider) != provider);
    }
    crate::provider_lake::replace_provider_live_snapshot_for_identity(
        ApiProvider::Custom,
        &provider,
        CatalogSnapshot::default(),
    );
}

/// Begin a provider refresh and invalidate older in-flight results.
///
/// Baseten additionally drops its prior in-memory roster because the same URL
/// can expose a different workspace catalog after an API-key change and the
/// endpoint supplies no safe account identifier for cache reuse.
pub fn begin_refresh(provider: &str) -> ProviderCatalogRefreshTicket {
    let provider = canonical_provider_scope(provider);
    let generation = if let Ok(mut generations) = REFRESH_GENERATIONS.write() {
        let generation = generations.entry(provider.clone()).or_default();
        *generation = generation.saturating_add(1);
        *generation
    } else {
        0
    };
    if is_account_scoped_provider(&provider) {
        forget_account_scoped_provider(&provider);
    }
    ProviderCatalogRefreshTicket {
        provider,
        generation,
    }
}

fn with_current_ticket<T>(
    ticket: &ProviderCatalogRefreshTicket,
    provider: &str,
    operation: impl FnOnce() -> T,
) -> Option<T> {
    let provider = canonical_provider_scope(provider);
    if ticket.provider != provider {
        return None;
    }
    let generations = REFRESH_GENERATIONS.read().ok()?;
    if generations.get(&ticket.provider).copied() != Some(ticket.generation) {
        return None;
    }
    // Keep the generation read guard alive through publication. A newer
    // `begin_refresh` needs the write lock, so it cannot slip between the
    // current-ticket check and this operation's cache/lake update.
    let result = operation();
    drop(generations);
    Some(result)
}

/// Record a successful refresh only if no newer refresh superseded it.
pub fn record_success_if_current(
    ticket: &ProviderCatalogRefreshTicket,
    delta: ProviderCatalogDelta,
) -> Option<CatalogStatus> {
    let provider = canonical_provider_scope(&delta.provider);
    with_current_ticket(ticket, &provider, || record_success(delta))
}

/// Record a failed refresh only if no newer refresh superseded it.
pub fn record_failure_if_current(
    ticket: &ProviderCatalogRefreshTicket,
    provider: &str,
    fingerprint: &str,
    reason: CatalogRefreshError,
) -> Option<CatalogStatus> {
    let provider = canonical_provider_scope(provider);
    with_current_ticket(ticket, &provider, || {
        record_failure(&provider, fingerprint, reason)
    })
}

/// Current freshness receipt for one exact provider/base-URL scope.
///
/// Runtime route resolution uses this independently from picker visibility:
/// stale or failed rows may remain selectable as an explicit fallback, but
/// their limits, capabilities, and prices are not treated as current endpoint
/// facts during execution.
pub fn status_for_scope(provider: &str, base_url: &str) -> CatalogStatus {
    let fingerprint = base_url_fingerprint(base_url);
    status_for_fingerprint(provider, &fingerprint)
}

/// Current freshness receipt when the caller already owns the endpoint
/// fingerprint (for example, an immutable usage-pricing receipt).
pub(crate) fn status_for_fingerprint(provider: &str, fingerprint: &str) -> CatalogStatus {
    let provider = canonical_provider_scope(provider);
    CACHE
        .read()
        .map(|cache| cache.status(&provider, fingerprint, now_unix()))
        .unwrap_or(CatalogStatus::Unknown)
}

/// Freeze the exact reviewed provider-live rate row fresh at CodeWhale's
/// pre-permit application-dispatch boundary.
///
/// Status, scope, model, source, and rates are all read beneath one `CACHE`
/// read guard. The returned value owns every fact needed by later auditing, so
/// completion-time code never re-opens mutable catalog or provider-lake state.
fn reviewed_provider_live_scope(
    provider: ApiProvider,
    provider_identity: &str,
    endpoint_fingerprint: &str,
) -> bool {
    match provider {
        ApiProvider::Openrouter => {
            provider_identity == ApiProvider::Openrouter.as_str()
                && endpoint_fingerprint
                    == base_url_fingerprint(crate::config::DEFAULT_OPENROUTER_BASE_URL)
        }
        ApiProvider::Custom => {
            codewhale_config::provider_setup_template(provider_identity)
                .is_some_and(|template| template.id == codewhale_config::BASETEN_TEMPLATE_ID)
                && endpoint_fingerprint == base_url_fingerprint(codewhale_config::BASETEN_BASE_URL)
        }
        _ => false,
    }
}

#[must_use]
pub(crate) fn fresh_provider_live_pricing_quote_at(
    provider: ApiProvider,
    provider_identity: &str,
    wire_model: &str,
    endpoint_fingerprint: &str,
    dispatched_at_unix: u64,
) -> Option<ProviderLivePricingQuote> {
    let provider_identity = canonical_provider_scope(provider_identity);
    let wire_model = wire_model.trim();
    let endpoint_fingerprint = endpoint_fingerprint.trim();
    if provider_identity.is_empty()
        || wire_model.is_empty()
        || !reviewed_provider_live_scope(provider, &provider_identity, endpoint_fingerprint)
    {
        return None;
    }

    let cache = CACHE.read().ok()?;
    if cache.status(&provider_identity, endpoint_fingerprint, dispatched_at_unix)
        != CatalogStatus::Fresh
    {
        return None;
    }
    let entry = cache.get(&provider_identity, endpoint_fingerprint)?;
    if entry.provider.trim() != provider_identity
        || entry.base_url_fingerprint.trim() != endpoint_fingerprint
        || entry.fetched_at > dispatched_at_unix
    {
        return None;
    }
    let offering = entry.offerings.iter().find(|offering| {
        offering.provider.trim() == provider_identity && offering.wire_model_id.trim() == wire_model
    })?;
    let pricing = OfferingPricing::from_catalog_offering(offering)?;
    if pricing.provider.trim() != provider_identity
        || pricing.wire_model_id.trim() != wire_model
        || pricing.currency != Currency::Usd
        || pricing.provenance != PricingProvenance::ProviderLive
        || pricing.effective_at != Some(entry.fetched_at)
        || pricing.endpoint_fingerprint.as_deref() != Some(endpoint_fingerprint)
        || pricing.input_per_million.is_none()
        || pricing.output_per_million.is_none()
    {
        return None;
    }
    ProviderLivePricingQuote::from_pricing(
        provider,
        &provider_identity,
        wire_model,
        endpoint_fingerprint,
        entry.fetched_at,
        &pricing,
    )
}

/// Record and atomically persist a successful provider refresh.
///
/// `ProviderCatalogCache::record_success` replaces the exact scope, so models
/// removed upstream disappear instead of accumulating forever.
pub fn record_success(mut delta: ProviderCatalogDelta) -> CatalogStatus {
    let provider = canonical_provider_scope(&delta.provider);
    delta.provider.clone_from(&provider);
    delta.offerings.retain_mut(|row| {
        if canonical_provider_scope(&row.provider) != provider {
            return false;
        }
        row.provider.clone_from(&provider);
        true
    });
    let fingerprint = delta.base_url_fingerprint.clone();
    let Ok(mut guard) = CACHE.write() else {
        return CatalogStatus::Unknown;
    };
    guard.record_success(delta, DEFAULT_PROVIDER_CATALOG_TTL_SECS);
    persist_scope(&guard, &provider, &fingerprint);
    publish_exact_scope(&guard, &provider, &fingerprint);
    CatalogStatus::Fresh
}

/// Record a typed failure while preserving and republishing prior rows for the
/// exact route scope.
pub fn record_failure(
    provider: &str,
    fingerprint: &str,
    reason: CatalogRefreshError,
) -> CatalogStatus {
    let provider = canonical_provider_scope(provider);
    let Ok(mut guard) = CACHE.write() else {
        return CatalogStatus::Failed { reason };
    };
    guard.record_failure(&provider, fingerprint, reason);
    persist_failure_scope(&mut guard, &provider, fingerprint, reason);
    publish_exact_scope(&guard, &provider, fingerprint);
    CatalogStatus::Failed { reason }
}

#[cfg(test)]
pub(crate) fn reset_cache_for_test() {
    if let Ok(mut cache) = CACHE.write() {
        *cache = ProviderCatalogCache::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, ProviderConfig, ProvidersConfig};
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use codewhale_config::catalog::{CatalogOffering, CatalogSource};

    fn delta(provider: &str, fingerprint: &str, ids: &[&str]) -> ProviderCatalogDelta {
        delta_at(provider, fingerprint, ids, now_unix())
    }

    fn delta_at(
        provider: &str,
        fingerprint: &str,
        ids: &[&str],
        fetched_at: u64,
    ) -> ProviderCatalogDelta {
        ProviderCatalogDelta {
            provider: provider.to_string(),
            base_url_fingerprint: fingerprint.to_string(),
            fetched_at,
            offerings: ids
                .iter()
                .map(|id| CatalogOffering {
                    provider: provider.to_string(),
                    wire_model_id: (*id).to_string(),
                    endpoint_key: "chat".to_string(),
                    source: CatalogSource::Live {
                        base_url_fingerprint: fingerprint.to_string(),
                        fetched_at,
                    },
                    ..CatalogOffering::default()
                })
                .collect(),
        }
    }

    #[test]
    fn success_replaces_scope_and_failure_preserves_last_rows_on_disk() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        if let Ok(mut cache) = CACHE.write() {
            *cache = ProviderCatalogCache::new();
        }

        assert_eq!(
            record_success(delta("openrouter", "fp", &["old"])),
            CatalogStatus::Fresh
        );
        assert_eq!(
            record_success(delta("openrouter", "fp", &["new"])),
            CatalogStatus::Fresh
        );
        assert!(matches!(
            record_failure("openrouter", "fp", CatalogRefreshError::RateLimited),
            CatalogStatus::Failed {
                reason: CatalogRefreshError::RateLimited
            }
        ));

        let loaded = load_from_disk().expect("persisted cache");
        let entry = loaded.get("openrouter", "fp").expect("OpenRouter scope");
        assert_eq!(entry.offerings.len(), 1);
        assert_eq!(entry.offerings[0].wire_model_id, "new");
        assert!(matches!(entry.status, CatalogStatus::Failed { .. }));
    }

    #[test]
    fn baseten_workspace_roster_is_session_only_and_clears_before_reauthentication() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let base_url = codewhale_config::BASETEN_BASE_URL;
        let fingerprint = base_url_fingerprint(base_url);
        record_success(delta(
            codewhale_config::BASETEN_TEMPLATE_ID,
            &fingerprint,
            &["workspace-a-only-model"],
        ));
        assert!(
            crate::provider_lake::all_catalog_models_for_provider_identity(
                ApiProvider::Custom,
                Some(codewhale_config::BASETEN_TEMPLATE_ID),
            )
            .contains(&"workspace-a-only-model".to_string())
        );
        assert!(
            load_from_disk().is_none_or(|cache| cache
                .get(codewhale_config::BASETEN_TEMPLATE_ID, &fingerprint)
                .is_none()),
            "an account-scoped Baseten roster must never be durable without a safe account id"
        );

        let mut custom = std::collections::HashMap::new();
        custom.insert(
            codewhale_config::BASETEN_TEMPLATE_ID.to_string(),
            ProviderConfig {
                kind: Some("openai-compatible".to_string()),
                base_url: Some(base_url.to_string()),
                model: Some(codewhale_config::BASETEN_DEFAULT_MODEL.to_string()),
                ..ProviderConfig::default()
            },
        );
        let config = Config {
            provider: Some(codewhale_config::BASETEN_TEMPLATE_ID.to_string()),
            providers: Some(ProvidersConfig {
                custom,
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        assert_eq!(maybe_load_persisted_cache_for_config(&config), 0);
        assert!(matches!(
            status_for_scope(codewhale_config::BASETEN_TEMPLATE_ID, base_url),
            CatalogStatus::Unknown
        ));
        assert!(
            !crate::provider_lake::all_catalog_models_for_provider_identity(
                ApiProvider::Custom,
                Some(codewhale_config::BASETEN_TEMPLATE_ID),
            )
            .contains(&"workspace-a-only-model".to_string()),
            "a new credential attempt must not see the previous workspace roster"
        );

        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();
    }

    #[test]
    fn superseded_refresh_ticket_cannot_publish_a_late_response() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let old = begin_refresh("openrouter");
        let current = begin_refresh("openrouter");
        assert!(
            record_success_if_current(&old, delta("openrouter", "fp", &["late-old-model"]))
                .is_none()
        );
        assert!(
            record_success_if_current(&current, delta("openrouter", "fp", &["current-model"]),)
                .is_some()
        );
        assert_eq!(
            CACHE
                .read()
                .expect("cache")
                .get("openrouter", "fp")
                .expect("current scope")
                .offerings[0]
                .wire_model_id,
            "current-model"
        );

        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();
    }

    #[test]
    fn current_ticket_holds_generation_gate_through_publication() {
        let ticket = begin_refresh("generation-barrier-provider");
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let publish_entered = std::sync::Arc::clone(&entered);
        let publish_release = std::sync::Arc::clone(&release);
        let publisher = std::thread::spawn(move || {
            with_current_ticket(&ticket, "generation-barrier-provider", || {
                publish_entered.wait();
                publish_release.wait();
            })
        });
        entered.wait();

        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let newer = std::thread::spawn(move || {
            started_tx.send(()).expect("signal refresh start");
            let next = begin_refresh("generation-barrier-provider");
            finished_tx.send(next).expect("signal refresh finish");
        });
        started_rx.recv().expect("new refresh thread started");
        assert!(
            finished_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err(),
            "a newer generation must wait until the accepted result finishes publication"
        );

        release.wait();
        assert!(publisher.join().expect("publisher thread").is_some());
        assert!(
            finished_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .is_ok()
        );
        newer.join().expect("newer refresh thread");
    }

    #[test]
    fn stale_process_snapshots_merge_exact_scopes_under_file_lock() {
        let _env = lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());

        let mut process_a = ProviderCatalogCache::new();
        process_a.record_success(delta("CustomA", "fp-a", &["upper-model"]), 60);
        persist_scope(&process_a, "CustomA", "fp-a");

        // Simulate another process that started before A wrote and therefore
        // has an empty/stale in-memory snapshot. Its scoped write must merge A
        // from disk rather than replacing the whole envelope.
        let mut process_b = ProviderCatalogCache::new();
        process_b.record_success(delta("customa", "fp-b", &["lower-model"]), 60);
        persist_scope(&process_b, "customa", "fp-b");

        let loaded = load_from_disk().expect("merged durable cache");
        assert_eq!(
            loaded
                .get("CustomA", "fp-a")
                .expect("case-sensitive upper scope")
                .offerings[0]
                .wire_model_id,
            "upper-model"
        );
        assert_eq!(
            loaded
                .get("customa", "fp-b")
                .expect("case-sensitive lower scope")
                .offerings[0]
                .wire_model_id,
            "lower-model"
        );
    }

    #[test]
    fn stale_process_failure_preserves_newer_durable_rows_for_the_same_scope() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        // Process B began with this old roster and still holds it in memory.
        let mut stale_process = ProviderCatalogCache::new();
        stale_process.record_success(delta_at("openrouter", "fp", &["old-model"], 1), 60);
        persist_scope(&stale_process, "openrouter", "fp");
        *CACHE.write().expect("cache") = stale_process;

        // Process A completes a newer successful refresh for the same scope.
        let mut newer_process = ProviderCatalogCache::new();
        newer_process.record_success(delta_at("openrouter", "fp", &["new-model"], 2), 60);
        persist_scope(&newer_process, "openrouter", "fp");

        // B then fails. Its failure status is current, but its old rows are
        // not: the transaction must retain A's newer durable roster.
        assert!(matches!(
            record_failure("openrouter", "fp", CatalogRefreshError::Network),
            CatalogStatus::Failed {
                reason: CatalogRefreshError::Network
            }
        ));
        let in_memory = CACHE.read().expect("cache");
        let entry = in_memory.get("openrouter", "fp").expect("failed scope");
        assert_eq!(entry.offerings[0].wire_model_id, "new-model");
        assert!(matches!(entry.status, CatalogStatus::Failed { .. }));
        drop(in_memory);

        let durable = load_from_disk().expect("durable cache");
        let entry = durable
            .get("openrouter", "fp")
            .expect("durable failed scope");
        assert_eq!(entry.offerings[0].wire_model_id, "new-model");
        assert!(matches!(entry.status, CatalogStatus::Failed { .. }));

        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();
    }

    #[test]
    fn oversized_cache_file_is_rejected_before_allocation() {
        let _env = lock_test_env();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        let path = cache_path().expect("cache path");
        fs::create_dir_all(path.parent().expect("catalog directory")).expect("catalog directory");
        fs::File::create(&path)
            .and_then(|file| file.set_len(MAX_CACHE_BYTES + 1))
            .expect("sparse oversized cache");
        assert!(load_from_disk().is_none());
    }

    #[test]
    fn bounded_persistence_evicts_failed_then_stale_scopes_and_keeps_exact_owner() {
        let mut cache = ProviderCatalogCache::new();
        cache.record_success(delta_at("failed", "fp", &["failed-model"], 10), 1_000);
        cache.record_failure("failed", "fp", CatalogRefreshError::Network);
        cache.record_success(delta_at("stale", "fp", &["stale-model"], 20), 1);
        cache.record_success(delta_at("fresh", "fp", &["fresh-model"], 30), 1_000);
        cache.record_success(delta_at("protected", "fp", &["protected-model"], 40), 1_000);

        let compacted = bounded_cache_for_persistence(
            cache,
            Some(("protected", "fp")),
            100,
            CachePersistenceLimits {
                max_bytes: u64::MAX,
                max_scopes: 2,
                max_rows: 100,
            },
        )
        .expect("bounded cache");

        assert!(compacted.get("protected", "fp").is_some());
        assert!(compacted.get("fresh", "fp").is_some());
        assert!(compacted.get("failed", "fp").is_none());
        assert!(compacted.get("stale", "fp").is_none());
    }

    #[test]
    fn bounded_persistence_evicts_whole_scopes_and_refuses_an_oversized_owner() {
        let mut cache = ProviderCatalogCache::new();
        cache.record_success(delta_at("protected", "fp", &["one", "two"], 40), 1_000);
        cache.record_success(delta_at("other", "fp", &["other"], 30), 1_000);
        let limits = CachePersistenceLimits {
            max_bytes: u64::MAX,
            max_scopes: 10,
            max_rows: 2,
        };

        let compacted =
            bounded_cache_for_persistence(cache.clone(), Some(("protected", "fp")), 50, limits)
                .expect("other scope can be evicted whole");
        assert_eq!(
            compacted
                .get("protected", "fp")
                .expect("protected roster")
                .offerings
                .len(),
            2
        );
        assert!(compacted.get("other", "fp").is_none());

        let mut oversized = cache;
        oversized.record_success(
            delta_at("protected", "fp", &["one", "two", "three"], 50),
            1_000,
        );
        assert!(
            bounded_cache_for_persistence(oversized, Some(("protected", "fp")), 50, limits,)
                .is_err(),
            "a provider roster must be refused, never partially persisted"
        );
    }

    #[test]
    fn bounded_cache_write_matches_read_limit_and_round_trips_after_compaction() {
        let directory = tempfile::tempdir().expect("cache directory");
        let path = directory.path().join(CACHE_FILE);
        let mut protected_only = ProviderCatalogCache::new();
        protected_only.record_success(delta_at("protected", "fp", &["protected-model"], 40), 1_000);
        let exact_bytes = persisted_envelope_len(&protected_only).expect("encoded length");
        let limits = CachePersistenceLimits {
            max_bytes: exact_bytes,
            max_scopes: 10,
            max_rows: 10,
        };

        let mut combined = protected_only;
        combined.record_success(
            delta_at(
                "evicted",
                "fp",
                &["this-entire-scope-does-not-fit-the-byte-bound"],
                30,
            ),
            1_000,
        );
        write_bounded_cache(&path, combined, Some(("protected", "fp")), limits)
            .expect("bounded disk write");

        assert!(fs::metadata(&path).expect("cache metadata").len() <= exact_bytes);
        let loaded = load_from_disk_unlocked_with_limit(&path, exact_bytes)
            .expect("bounded cache must remain readable under the same cap");
        assert!(loaded.get("protected", "fp").is_some());
        assert!(loaded.get("evicted", "fp").is_none());
    }

    #[test]
    fn baseten_alias_roster_is_session_only_and_keeps_exact_ownership() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();

        let alias = "base-ten";
        let fingerprint = base_url_fingerprint(codewhale_config::BASETEN_BASE_URL);
        record_success(delta(alias, &fingerprint, &["alias-workspace-model"]));

        assert!(
            crate::provider_lake::all_catalog_models_for_provider_identity(
                ApiProvider::Custom,
                Some(alias),
            )
            .contains(&"alias-workspace-model".to_string())
        );
        assert!(
            !crate::provider_lake::all_catalog_models_for_provider_identity(
                ApiProvider::Custom,
                Some(codewhale_config::BASETEN_TEMPLATE_ID),
            )
            .contains(&"alias-workspace-model".to_string()),
            "a reviewed schema alias must not collapse distinct exact table ownership"
        );
        assert!(
            load_from_disk().is_none_or(|cache| cache.get(alias, &fingerprint).is_none()),
            "every Baseten schema alias must remain session-only"
        );

        reset_cache_for_test();
        crate::provider_lake::clear_live_snapshot();
    }

    #[test]
    fn different_base_url_fingerprints_do_not_share_rows() {
        let mut cache = ProviderCatalogCache::new();
        cache.record_success(delta("baseten", "one", &["model-one"]), 60);
        cache.record_success(delta("baseten", "two", &["model-two"]), 60);
        assert_eq!(
            cache.get("baseten", "one").unwrap().offerings[0].wire_model_id,
            "model-one"
        );
        assert_eq!(
            cache.get("baseten", "two").unwrap().offerings[0].wire_model_id,
            "model-two"
        );
    }

    #[test]
    fn missing_cache_for_changed_base_url_clears_the_previous_provider_partition() {
        let _live = crate::provider_lake::lock_live_snapshot();
        crate::provider_lake::clear_live_snapshot();
        let mut cache = ProviderCatalogCache::new();
        cache.record_success(delta("baseten", "old-fp", &["old-endpoint-model"]), 60);

        assert_eq!(publish_exact_scope(&cache, "baseten", "old-fp"), 1);
        assert_eq!(
            crate::provider_lake::all_catalog_models_for_provider_identity(
                crate::config::ApiProvider::Custom,
                Some("baseten"),
            ),
            vec!["old-endpoint-model".to_string()]
        );

        assert_eq!(publish_exact_scope(&cache, "baseten", "new-fp"), 0);
        let after_switch = crate::provider_lake::all_catalog_models_for_provider_identity(
            crate::config::ApiProvider::Custom,
            Some("baseten"),
        );
        assert!(
            !after_switch.contains(&"old-endpoint-model".to_string()),
            "rows from the old Baseten endpoint must not survive a fingerprint change"
        );
        assert!(
            after_switch.contains(&codewhale_config::BASETEN_DEFAULT_MODEL.to_string()),
            "the exact provider should fall back to its offline seed"
        );
        crate::provider_lake::clear_live_snapshot();
    }

    #[test]
    fn disk_reload_rehydrates_and_exposes_six_hundred_openrouter_models() {
        let _env = lock_test_env();
        let _live = crate::provider_lake::lock_live_snapshot();
        let home = tempfile::tempdir().expect("home");
        let _home = EnvVarGuard::set("CODEWHALE_HOME", home.path());
        crate::provider_lake::clear_live_snapshot();
        if let Ok(mut cache) = CACHE.write() {
            *cache = ProviderCatalogCache::new();
        }

        let config = Config {
            provider: Some("openrouter".to_string()),
            providers: Some(ProvidersConfig {
                openrouter: ProviderConfig {
                    base_url: Some("https://synthetic.openrouter.invalid/api/v1".to_string()),
                    ..ProviderConfig::default()
                },
                ..ProvidersConfig::default()
            }),
            ..Config::default()
        };
        let provider = config.provider_identity_for(config.api_provider());
        let fingerprint = base_url_fingerprint(&config.deepseek_base_url());
        let fetched_at = now_unix();
        let ids: Vec<String> = (0..600)
            .map(|index| format!("synthetic/openrouter-model-{index:03}"))
            .collect();
        let status = record_success(ProviderCatalogDelta {
            provider: provider.clone(),
            base_url_fingerprint: fingerprint,
            fetched_at,
            offerings: ids
                .iter()
                .map(|id| CatalogOffering {
                    provider: provider.clone(),
                    wire_model_id: id.clone(),
                    endpoint_key: "chat".to_string(),
                    source: CatalogSource::Live {
                        base_url_fingerprint: base_url_fingerprint(&config.deepseek_base_url()),
                        fetched_at,
                    },
                    ..CatalogOffering::default()
                })
                .collect(),
        });
        assert_eq!(status, CatalogStatus::Fresh);
        assert!(cache_path().is_some_and(|path| path.is_file()));
        assert_eq!(
            crate::provider_lake::all_catalog_models_for_provider(ApiProvider::Openrouter),
            ids,
            "the string compatibility publisher must retain built-in OpenRouter ownership"
        );
        assert!(
            crate::provider_lake::all_catalog_models_for_provider_identity(
                ApiProvider::Custom,
                Some("openrouter"),
            )
            .is_empty(),
            "built-in OpenRouter rows must not enter the custom namespace"
        );

        // Simulate a new process: remove both in-memory owners, then republish
        // only through the durable startup load path.
        if let Ok(mut cache) = CACHE.write() {
            *cache = ProviderCatalogCache::new();
        }
        crate::provider_lake::clear_live_snapshot();

        assert_eq!(maybe_load_persisted_cache_for_config(&config), 600);
        let visible =
            crate::provider_lake::all_catalog_models_for_provider(ApiProvider::Openrouter);
        assert_eq!(visible.len(), 600);
        assert_eq!(visible.first(), ids.first());
        assert_eq!(visible.last(), ids.last());

        if let Ok(mut cache) = CACHE.write() {
            *cache = ProviderCatalogCache::new();
        }
        crate::provider_lake::clear_live_snapshot();
    }
}
