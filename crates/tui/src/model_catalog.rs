//! Offline model metadata catalog (#3072).
//!
//! This module adds a secret-free metadata layer in front of the legacy model
//! tables. It is intentionally conservative: startup reads a local cache plus a
//! bundled snapshot, never performs a network refresh, and only overrides a
//! legacy fact when the active catalog entry actually carries that field.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

const BUNDLED_CATALOG_JSON: &str = include_str!("../assets/model_catalog.bundled.json");
const OPENROUTER_CACHE_FILE: &str = "openrouter.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MetadataProvenance {
    ProviderApi,
    Bundled,
    UserOverride,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_usd_per_million: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_parameters: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model_id: Option<String>,
    #[serde(default)]
    pub provenance: MetadataProvenance,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogCache {
    pub schema_version: u32,
    pub source: String,
    pub fetched_at: DateTime<Utc>,
    pub ttl_secs: u64,
    #[serde(default)]
    pub entries: BTreeMap<String, CatalogEntry>,
}

impl CatalogCache {
    #[must_use]
    pub fn is_stale(&self, now: DateTime<Utc>) -> bool {
        if now <= self.fetched_at {
            return false;
        }
        let ttl = Duration::seconds(self.ttl_secs.min(i64::MAX as u64) as i64);
        now.signed_duration_since(self.fetched_at) > ttl
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MergedCatalog {
    user_overrides: BTreeMap<String, CatalogEntry>,
    provider_cache: Option<CatalogCache>,
    bundled: CatalogCache,
    now: DateTime<Utc>,
}

impl MergedCatalog {
    pub(crate) fn from_sources(
        user_overrides: BTreeMap<String, CatalogEntry>,
        provider_cache: Option<CatalogCache>,
        bundled: CatalogCache,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            user_overrides,
            provider_cache,
            bundled,
            now,
        }
    }

    #[must_use]
    pub(crate) fn resolve(&self, model: &str) -> Option<&CatalogEntry> {
        if let Some(entry) = entry_for(&self.user_overrides, model) {
            return Some(entry);
        }
        if let Some(provider_cache) = self
            .provider_cache
            .as_ref()
            .filter(|cache| !cache.is_stale(self.now))
            && let Some(entry) = entry_for(&provider_cache.entries, model)
        {
            return Some(entry);
        }
        entry_for(&self.bundled.entries, model)
    }
}

fn entry_for<'a>(
    entries: &'a BTreeMap<String, CatalogEntry>,
    model: &str,
) -> Option<&'a CatalogEntry> {
    entries.get(model).or_else(|| {
        let lower = model.to_lowercase();
        (lower != model).then(|| entries.get(&lower)).flatten()
    })
}

fn active_catalog() -> &'static RwLock<MergedCatalog> {
    static ACTIVE: OnceLock<RwLock<MergedCatalog>> = OnceLock::new();
    ACTIVE.get_or_init(|| {
        RwLock::new(MergedCatalog::from_sources(
            BTreeMap::new(),
            load_cached(),
            bundled_catalog(),
            Utc::now(),
        ))
    })
}

#[must_use]
pub fn resolved_entry(model: &str) -> Option<CatalogEntry> {
    active_catalog()
        .read()
        .ok()
        .and_then(|catalog| catalog.resolve(model).cloned())
}

#[must_use]
pub fn resolved_context_window(model: &str) -> Option<u32> {
    resolved_entry(model).and_then(|entry| entry.context_window)
}

#[must_use]
pub fn resolved_max_output(model: &str) -> Option<u32> {
    resolved_entry(model).and_then(|entry| entry.max_output)
}

#[must_use]
pub fn resolved_supports_reasoning(model: &str) -> Option<bool> {
    resolved_entry(model).and_then(|entry| entry.supports_reasoning)
}

#[must_use]
#[cfg_attr(test, allow(dead_code))]
pub fn resolved_usd_pricing(model: &str) -> Option<(f64, f64)> {
    let entry = resolved_entry(model)?;
    Some((entry.input_usd_per_million?, entry.output_usd_per_million?))
}

pub fn bundled_catalog() -> CatalogCache {
    serde_json::from_str(BUNDLED_CATALOG_JSON).expect("bundled model catalog must parse")
}

fn catalog_cache_read_path() -> Result<PathBuf> {
    Ok(codewhale_config::resolve_state_dir("catalog")?.join(OPENROUTER_CACHE_FILE))
}

pub fn load_cached() -> Option<CatalogCache> {
    let path = catalog_cache_read_path().ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

#[cfg(test)]
static TEST_CATALOG_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

#[cfg(test)]
pub(crate) fn test_catalog_lock() -> std::sync::MutexGuard<'static, ()> {
    TEST_CATALOG_LOCK.lock().expect("model catalog test lock")
}

#[cfg(test)]
pub(crate) struct ActiveCatalogGuard {
    previous: MergedCatalog,
}

#[cfg(test)]
impl Drop for ActiveCatalogGuard {
    fn drop(&mut self) {
        let mut active = active_catalog().write().expect("active catalog write lock");
        *active = self.previous.clone();
    }
}

#[cfg(test)]
pub(crate) fn replace_active_catalog_for_test(catalog: MergedCatalog) -> ActiveCatalogGuard {
    let mut active = active_catalog().write().expect("active catalog write lock");
    let previous = active.clone();
    *active = catalog;
    ActiveCatalogGuard { previous }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, context_window: u32, provenance: MetadataProvenance) -> CatalogEntry {
        CatalogEntry {
            id: id.to_string(),
            context_window: Some(context_window),
            max_output: Some(context_window / 2),
            supports_reasoning: Some(false),
            input_usd_per_million: None,
            output_usd_per_million: None,
            modalities: Vec::new(),
            supported_parameters: Vec::new(),
            provider_model_id: None,
            provenance,
        }
    }

    fn cache(
        fetched_at: DateTime<Utc>,
        ttl_secs: u64,
        entries: BTreeMap<String, CatalogEntry>,
    ) -> CatalogCache {
        CatalogCache {
            schema_version: 1,
            source: "test".to_string(),
            fetched_at,
            ttl_secs,
            entries,
        }
    }

    #[test]
    fn bundled_snapshot_parses_and_is_nonempty() {
        let bundled = bundled_catalog();
        assert_eq!(bundled.schema_version, 1);
        assert!(!bundled.entries.is_empty());
        assert_eq!(
            bundled.entries["deepseek-v4-pro"].provenance,
            MetadataProvenance::Bundled
        );
    }

    #[test]
    fn merge_order_is_user_override_then_provider_then_bundled() {
        let now = Utc::now();
        let mut bundled_entries = BTreeMap::new();
        bundled_entries.insert(
            "sample/model".to_string(),
            entry("sample/model", 1_000, MetadataProvenance::Bundled),
        );
        let bundled = cache(now, 3600, bundled_entries);

        let mut provider_entries = BTreeMap::new();
        provider_entries.insert(
            "sample/model".to_string(),
            entry("sample/model", 2_000, MetadataProvenance::ProviderApi),
        );
        let provider_cache = cache(now, 3600, provider_entries);

        let mut override_entries = BTreeMap::new();
        override_entries.insert(
            "sample/model".to_string(),
            entry("sample/model", 3_000, MetadataProvenance::UserOverride),
        );

        let merged =
            MergedCatalog::from_sources(override_entries, Some(provider_cache), bundled, now);
        let resolved = merged.resolve("sample/model").expect("resolved");
        assert_eq!(resolved.context_window, Some(3_000));
        assert_eq!(resolved.provenance, MetadataProvenance::UserOverride);
    }

    #[test]
    fn stale_cache_is_ignored_for_facts() {
        let now = Utc::now();
        let mut bundled_entries = BTreeMap::new();
        bundled_entries.insert(
            "sample/model".to_string(),
            entry("sample/model", 1_000, MetadataProvenance::Bundled),
        );
        let bundled = cache(now, 3600, bundled_entries);

        let mut provider_entries = BTreeMap::new();
        provider_entries.insert(
            "sample/model".to_string(),
            entry("sample/model", 9_000, MetadataProvenance::ProviderApi),
        );
        let provider_cache = cache(now - Duration::seconds(10), 1, provider_entries);
        assert!(provider_cache.is_stale(now));

        let merged =
            MergedCatalog::from_sources(BTreeMap::new(), Some(provider_cache), bundled, now);
        let resolved = merged.resolve("sample/model").expect("resolved");
        assert_eq!(resolved.context_window, Some(1_000));
        assert_eq!(resolved.provenance, MetadataProvenance::Bundled);
    }

    #[test]
    fn cache_roundtrip_serializes_no_secret_fields() {
        let mut entries = BTreeMap::new();
        entries.insert(
            "sample/model".to_string(),
            CatalogEntry {
                input_usd_per_million: Some(0.25),
                output_usd_per_million: Some(1.25),
                ..entry("sample/model", 32_000, MetadataProvenance::ProviderApi)
            },
        );
        let cache = cache(Utc::now(), 60, entries);
        let json = serde_json::to_string_pretty(&cache).expect("serialize");
        let lowered = json.to_lowercase();
        for forbidden in ["api_key", "authorization", "token", "secret"] {
            assert!(
                !lowered.contains(forbidden),
                "cache JSON must not contain auth field {forbidden}: {json}"
            );
        }
        let parsed: CatalogCache = serde_json::from_str(&json).expect("roundtrip");
        assert_eq!(parsed.entries.len(), 1);
    }
}
