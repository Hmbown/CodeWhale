//! Configured provider/model lake facade (#3830, Wave 5b / #4188).
//!
//! Single seam over the Models.dev catalog layers and the configured-provider
//! predicate shared with `/provider`. Precedence is **provider-scoped live >
//! live Models.dev > bundled offline snapshot > legacy hardcoded fallback**.
//! Pickers, hotbar route slots, [`crate::model_inventory::ModelInventory`],
//! slash completions, and subagent validation should read model lists from here.
//!
//! [`crate::config::model_completion_names_for_provider`] is retained only as a
//! compatibility fallback for CodeWhale-only / local providers that Models.dev
//! does not represent (and for unbundled gateways until the live catalog covers
//! them).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot, bundled_catalog_offerings};

use crate::codex_model_cache;
use crate::config::{
    ApiProvider, Config, model_completion_names_for_provider, opencode_go_chat_model_id,
    provider_is_configured_for_active,
};

static BUNDLED_SNAPSHOT: std::sync::OnceLock<CatalogSnapshot> = std::sync::OnceLock::new();

/// Source tag for live-catalog rows. Models.dev is a cross-provider catalog
/// that serves as the primary live layer; per-provider refreshes (e.g.
/// TelecomJS `/v1/models`) are a secondary layer that must coexist alongside
/// Models.dev rows without being wiped by a Models.dev refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiveSource {
    /// The cross-provider Models.dev catalog refresh.
    ModelsDev,
    /// A per-provider `/v1/models` catalog refresh (e.g. TelecomJS TokenHub).
    PerProvider,
}

/// Optional live catalog snapshot(s), source-scoped (#4188 race fix).
///
/// Models.dev and every provider fetch maintain distinct partitions of live
/// rows. A Models.dev refresh replaces only Models.dev-sourced rows; a
/// per-provider merge adds/replaces only that provider's rows. This prevents a
/// later Models.dev `set_live_snapshot` from erasing TelecomJS rows and keeps
/// independent provider refreshes from erasing each other.
static LIVE_SNAPSHOT: RwLock<LiveSnapshotPartitions> = RwLock::new(LiveSnapshotPartitions {
    models_dev: None,
    per_provider: BTreeMap::new(),
});

/// Internal partition map: one Models.dev snapshot plus one snapshot per
/// provider-specific live fetch.
#[derive(Default)]
struct LiveSnapshotPartitions {
    models_dev: Option<CatalogSnapshot>,
    per_provider: BTreeMap<String, CatalogSnapshot>,
}

impl LiveSnapshotPartitions {
    /// Collect all live rows from every partition into a single flat snapshot.
    fn flattened(&self) -> Option<CatalogSnapshot> {
        if self.models_dev.is_none() && self.per_provider.is_empty() {
            return None;
        }

        // Merge by (provider, wire_model_id); provider-scoped rows win on
        // collision because they came from that gateway's own live endpoint.
        let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
        if let Some(models_dev) = &self.models_dev {
            for row in &models_dev.offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
        }
        for provider_snapshot in self.per_provider.values() {
            for row in &provider_snapshot.offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
        }
        Some(CatalogSnapshot {
            offerings: merged.into_values().collect(),
        })
    }
}

fn offerings_by_provider(
    offerings: Vec<CatalogOffering>,
) -> BTreeMap<String, Vec<CatalogOffering>> {
    let mut grouped = BTreeMap::new();
    for offering in offerings {
        grouped
            .entry(offering.provider.trim().to_ascii_lowercase())
            .or_insert_with(Vec::new)
            .push(offering);
    }
    grouped
}

/// Generation stamp for the live snapshot. Bumped (under the `LIVE_SNAPSHOT`
/// write lock) by [`set_live_snapshot`], [`merge_live_offerings`], and
/// [`clear_live_snapshot`] so the memoized merged snapshot below can detect
/// staleness without re-merging.
static LIVE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Memoized result of [`merged_snapshot`], tagged with the `LIVE_GENERATION`
/// it was computed from. Re-merging ~5,700 offerings per call made every
/// `/model` open pay a multi-second, UI-thread-blocking cost; the merge result
/// only changes when the live snapshot changes, so cache it.
static MERGED_CACHE: RwLock<Option<(u64, Arc<CatalogSnapshot>)>> = RwLock::new(None);

fn bundled_snapshot() -> &'static CatalogSnapshot {
    BUNDLED_SNAPSHOT.get_or_init(|| CatalogSnapshot {
        offerings: bundled_catalog_offerings(),
    })
}

/// Remove catalog rows that cannot use the selected provider's wire protocol.
///
/// OpenCode Go publishes one `/models` roster for both Chat Completions and
/// Anthropic Messages. The `OpencodeGo` route is Chat-only, so sanitize both
/// saved/live snapshots and the bundled fallback at the lake boundary. This is
/// deliberately downstream of every publisher so stale cached rows cannot
/// bypass the client-side live-fetch filter.
fn apply_provider_model_cutlines(mut snapshot: CatalogSnapshot) -> CatalogSnapshot {
    // `ApiProvider::parse` scans every provider and alias list per call; the
    // distinct provider strings in a catalog are few, so resolve each distinct
    // string once instead of once per offering (boot-path profiles showed
    // this loop as the largest post-parse compute block).
    let mut resolved: std::collections::HashMap<String, Option<ApiProvider>> =
        std::collections::HashMap::new();
    snapshot.offerings = snapshot
        .offerings
        .into_iter()
        .filter_map(|mut offering| {
            let parsed = *resolved
                .entry(offering.provider.clone())
                .or_insert_with(|| ApiProvider::parse(&offering.provider));
            if parsed == Some(ApiProvider::OpencodeGo) {
                let canonical = opencode_go_chat_model_id(&offering.wire_model_id)?;
                offering.provider = ApiProvider::OpencodeGo.as_str().to_string();
                offering.wire_model_id = canonical.to_string();
            }
            Some(offering)
        })
        .collect();
    snapshot
}

/// Set the live-catalog snapshot for a given source (#4188 race fix).
///
/// Source-scoped: a Models.dev refresh replaces only Models.dev-sourced rows;
/// a per-provider refresh replaces only the layers for providers represented
/// in that snapshot. Other providers and sources are preserved. This
/// eliminates the race where a Models.dev `set_live_snapshot` would erase
/// TelecomJS rows merged earlier.
pub fn set_live_snapshot(snapshot: CatalogSnapshot, source: LiveSource) {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        let snapshot = apply_provider_model_cutlines(snapshot);
        let changed = match source {
            LiveSource::ModelsDev => {
                guard.models_dev = Some(snapshot);
                true
            }
            LiveSource::PerProvider => {
                let grouped = offerings_by_provider(snapshot.offerings);
                let changed = !grouped.is_empty();
                for (provider, offerings) in grouped {
                    guard
                        .per_provider
                        .insert(provider, CatalogSnapshot { offerings });
                }
                changed
            }
        };
        // Invalidate the memoized merged snapshot while still holding the
        // write lock so no reader can cache the old merge against the new
        // generation.
        if changed {
            LIVE_GENERATION.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// Clear all live snapshots (both Models.dev and per-provider partitions).
/// Used by tests and shutdown paths that need a full reset.
#[allow(dead_code)]
pub fn clear_live_snapshot() {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        guard.models_dev = None;
        guard.per_provider.clear();
        LIVE_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}

/// Merge additional live offerings into provider-scoped live partitions (#4188).
///
/// Unlike [`set_live_snapshot`] for `LiveSource::PerProvider` (which replaces
/// each represented provider's partition), this merges new rows by
/// `(provider, wire_model_id)` identity within that provider's partition,
/// preserving every other provider and the Models.dev partition. This is used
/// by provider catalog refreshes (e.g. TelecomJS `/v1/models`) that need to
/// coexist with the cross-provider Models.dev live layer.
pub fn merge_live_offerings(new_offerings: Vec<CatalogOffering>) {
    if new_offerings.is_empty() {
        return;
    }
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        for (provider, new_rows) in offerings_by_provider(new_offerings) {
            let existing = guard.per_provider.remove(&provider).unwrap_or_default();
            let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
            for row in existing.offerings {
                merged.insert((row.provider.clone(), row.wire_model_id.clone()), row);
            }
            for row in new_rows {
                merged.insert((row.provider.clone(), row.wire_model_id.clone()), row);
            }
            guard.per_provider.insert(
                provider,
                CatalogSnapshot {
                    offerings: merged.into_values().collect(),
                },
            );
        }
        LIVE_GENERATION.fetch_add(1, Ordering::SeqCst);
    }
}

/// Which live partition currently holds `(provider, wire_model_id)`, if any.
///
/// Per-provider `/models` rows win on collision, matching merge precedence.
/// Pricing uses this so a Models.dev capabilities overlay is never treated as
/// a rate source (#5241).
#[must_use]
pub fn live_catalog_origin(provider: ApiProvider, wire_model_id: &str) -> Option<LiveSource> {
    let catalog_id = catalog_provider_id(provider);
    let needle = wire_model_id.trim();
    if needle.is_empty() {
        return None;
    }
    let Ok(guard) = LIVE_SNAPSHOT.read() else {
        return None;
    };
    let matches = |row: &CatalogOffering| {
        row.provider.eq_ignore_ascii_case(catalog_id)
            && row.wire_model_id.eq_ignore_ascii_case(needle)
    };
    if guard
        .per_provider
        .values()
        .any(|snap| snap.offerings.iter().any(matches))
    {
        return Some(LiveSource::PerProvider);
    }
    if guard
        .models_dev
        .as_ref()
        .is_some_and(|snap| snap.offerings.iter().any(matches))
    {
        return Some(LiveSource::ModelsDev);
    }
    None
}

/// Serialize tests that mutate the process-wide live snapshot.
#[cfg(test)]
pub(crate) fn lock_live_snapshot() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The merged catalog snapshot: live rows override bundled rows on
/// `(provider, wire_model_id)` identity (#4188). When no live snapshot is
/// present, this is just the offline bundled snapshot. Per-provider live rows
/// override Models.dev live rows on collision (gateway-specific wins over
/// cross-provider).
///
/// Memoized: the merge is recomputed only after a live-layer mutation bumps
/// `LIVE_GENERATION`; every other call returns the cached `Arc` (the picker
/// calls this per row, so it must be cheap).
fn merged_snapshot() -> Arc<CatalogSnapshot> {
    let generation = LIVE_GENERATION.load(Ordering::SeqCst);
    if let Ok(guard) = MERGED_CACHE.read()
        && let Some((cached_generation, cached)) = guard.as_ref()
        && *cached_generation == generation
    {
        return Arc::clone(cached);
    }
    let merged = Arc::new(compute_merged_snapshot());
    if let Ok(mut guard) = MERGED_CACHE.write() {
        // `generation` was sampled before the live snapshot was read, so a
        // concurrent set/clear leaves this entry stale-tagged and the next
        // reader recomputes; the merge itself is always internally consistent.
        *guard = Some((generation, Arc::clone(&merged)));
    }
    merged
}

/// Uncached merge (see [`merged_snapshot`] for the caching seam).
fn compute_merged_snapshot() -> CatalogSnapshot {
    let live = LIVE_SNAPSHOT
        .read()
        .ok()
        .and_then(|guard| guard.flattened());
    let merged = match live {
        None => bundled_snapshot().clone(),
        Some(live) => {
            let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
            for row in &bundled_snapshot().offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
            for row in &live.offerings {
                merged.insert(
                    (row.provider.clone(), row.wire_model_id.clone()),
                    row.clone(),
                );
            }
            CatalogSnapshot {
                offerings: merged.into_values().collect(),
            }
        }
    };
    apply_provider_model_cutlines(merged)
}

/// Maps an [`ApiProvider`] to its bundled-catalog provider id.
fn catalog_provider_id(provider: ApiProvider) -> &'static str {
    match provider {
        ApiProvider::DeepseekCN | ApiProvider::DeepseekAnthropic => "deepseek",
        ApiProvider::SiliconflowCn => "siliconflow",
        _ => provider.as_str(),
    }
}

fn push_unique_model(models: &mut Vec<String>, model: &str) {
    let model = model.trim();
    if model.is_empty() {
        return;
    }
    if !models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(model))
    {
        models.push(model.to_string());
    }
}

fn catalog_models_from_offerings<'a>(
    offerings: impl IntoIterator<Item = &'a CatalogOffering>,
) -> Vec<String> {
    let mut rows: Vec<_> = offerings.into_iter().collect();
    rows.sort_by(|left, right| {
        right
            .default_for_provider
            .cmp(&left.default_for_provider)
            .then_with(|| left.wire_model_id.cmp(&right.wire_model_id))
    });
    let mut models = Vec::new();
    for row in rows {
        push_unique_model(&mut models, &row.wire_model_id);
    }
    models
}

/// Tags from the provider's own live `/v1/models` partition.
///
/// Models.dev rows must not satisfy a LOCAL default (Ollama). This reads only
/// the PerProvider snapshot so a cross-provider catalog cannot costume a
/// machine that has not answered with its own tags.
#[must_use]
pub fn live_per_provider_models(provider: ApiProvider) -> Vec<String> {
    let catalog_id = catalog_provider_id(provider).to_ascii_lowercase();
    let Ok(guard) = LIVE_SNAPSHOT.read() else {
        return Vec::new();
    };
    let Some(snapshot) = guard.per_provider.get(&catalog_id) else {
        return Vec::new();
    };
    catalog_models_from_offerings(&snapshot.offerings)
}

/// Catalog-backed model ids for one provider (#4188).
///
/// Precedence: live Models.dev rows (when published) override bundled offline
/// rows on `(provider, wire_model_id)`; if the merged catalog still has no rows
/// for the provider, fall back to
/// [`crate::config::model_completion_names_for_provider`] so CodeWhale-only /
/// local providers (and gateways not yet in the offline seed) keep defaults.
#[must_use]
pub fn all_catalog_models_for_provider(provider: ApiProvider) -> Vec<String> {
    // ChatGPT OAuth availability is account-scoped. A generic OpenAI or
    // Models.dev catalog is not evidence that a model can be routed through
    // the Codex backend, so this provider owns a separate secret-free source.
    if provider == ApiProvider::OpenaiCodex {
        return codex_model_cache::model_roster().model_ids();
    }

    let catalog_id = catalog_provider_id(provider);
    let merged = merged_snapshot();
    let mut models = catalog_models_from_offerings(merged.offerings_for_provider(catalog_id));
    if models.is_empty() {
        for model in model_completion_names_for_provider(provider) {
            push_unique_model(&mut models, model);
        }
    }
    models
}

/// Look up a merged-catalog offering for `(provider, wire_model_id)` (#4115).
///
/// Returns the live-over-bundled row when present so picker metadata (context,
/// pricing, tools, reasoning, freshness) can be projected without a second
/// catalog walk. `None` for CodeWhale-only / legacy-fallback ids that have no
/// Models.dev row.
#[must_use]
pub fn catalog_offering_for_model(
    provider: ApiProvider,
    wire_model_id: &str,
) -> Option<CatalogOffering> {
    if provider == ApiProvider::OpenaiCodex {
        return None;
    }
    let catalog_id = catalog_provider_id(provider);
    let needle = wire_model_id.trim();
    if needle.is_empty() {
        return None;
    }
    merged_snapshot()
        .offerings_for_provider(catalog_id)
        .into_iter()
        .find(|row| row.wire_model_id.eq_ignore_ascii_case(needle))
        .cloned()
}

/// Look up the **bundled-snapshot** offering for `(provider, wire_model_id)`,
/// ignoring any live rows merged over it.
///
/// Pricing uses this as an honest fallback when a live row cannot be verified as
/// authoritative for the endpoint being priced (stale fetch, or a fetch from a
/// different base URL). The bundled snapshot is a published Models.dev seed with
/// no endpoint scoping, so it is authoritative for the model without needing a
/// freshness proof — degrading to it is strictly more truthful than billing
/// against an unverified live rate (#4318).
#[must_use]
pub fn bundled_catalog_offering_for_model(
    provider: ApiProvider,
    wire_model_id: &str,
) -> Option<CatalogOffering> {
    if provider == ApiProvider::OpenaiCodex {
        return None;
    }
    let catalog_id = catalog_provider_id(provider);
    let needle = wire_model_id.trim();
    if needle.is_empty() {
        return None;
    }
    bundled_snapshot()
        .offerings_for_provider(catalog_id)
        .into_iter()
        .find(|row| row.wire_model_id.eq_ignore_ascii_case(needle))
        .cloned()
}

/// Count of merged-catalog models for one provider (catalog view / dashboard).
#[must_use]
pub fn catalog_model_count_for_provider(provider: ApiProvider) -> usize {
    all_catalog_models_for_provider(provider).len()
}

/// Providers the user has set up — active provider, working credentials/OAuth,
/// or an explicit `[providers.<name>]` entry (#3830).
#[must_use]
pub fn configured_providers(config: &Config, active: ApiProvider) -> Vec<ApiProvider> {
    ApiProvider::sorted_for_display()
        .into_iter()
        .filter(|provider| provider_is_configured_for_active(config, *provider, active))
        .collect()
}

/// Catalog models for providers that qualify as configured for `active`.
#[must_use]
pub fn models_for_provider(
    config: &Config,
    active: ApiProvider,
    provider: ApiProvider,
) -> Vec<String> {
    if provider_is_configured_for_active(config, provider, active) {
        all_catalog_models_for_provider(provider)
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TOGETHER_FLASH_MODEL, DEFAULT_TOGETHER_MODEL};
    use codewhale_config::catalog::CatalogSource;

    #[test]
    fn together_catalog_includes_flash_from_bundled_asset() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let models = all_catalog_models_for_provider(ApiProvider::Together);
        assert!(
            models.contains(&DEFAULT_TOGETHER_MODEL.to_string()),
            "missing Together pro: {models:?}"
        );
        assert!(
            models.contains(&DEFAULT_TOGETHER_FLASH_MODEL.to_string()),
            "missing Together flash: {models:?}"
        );
    }

    #[test]
    fn configured_providers_matches_provider_predicate() {
        let _env_lock = crate::test_support::lock_test_env();
        let tmp = tempfile::tempdir().expect("tempdir");
        let _auth_file = crate::test_support::EnvVarGuard::set(
            "OPENAI_CODEX_AUTH_FILE",
            tmp.path().join("missing-auth.json"),
        );
        let _openai_token = crate::test_support::EnvVarGuard::remove("OPENAI_CODEX_ACCESS_TOKEN");
        let _codex_token = crate::test_support::EnvVarGuard::remove("CODEX_ACCESS_TOKEN");
        let config = Config::default();
        let active = ApiProvider::Deepseek;
        let expected: Vec<_> = ApiProvider::sorted_for_display()
            .into_iter()
            .filter(|provider| {
                crate::config::provider_is_configured_for_active(&config, *provider, active)
            })
            .collect();
        assert_eq!(configured_providers(&config, active), expected);
    }

    #[test]
    fn models_for_provider_filters_unconfigured_gateways() {
        let _env_lock = crate::test_support::lock_test_env();
        let _together = crate::test_support::EnvVarGuard::remove("TOGETHER_API_KEY");
        let config = Config::default();
        assert!(
            models_for_provider(&config, ApiProvider::Deepseek, ApiProvider::Together).is_empty()
        );
        assert!(
            !models_for_provider(&config, ApiProvider::Deepseek, ApiProvider::Deepseek).is_empty()
        );
    }

    /// #4116 CRITICAL (no-narrowing guarantee for the migrated consumer): the
    /// catalog-backed facade must return a NON-EMPTY enumeration for every
    /// provider that has a non-empty legacy `model_completion_names_for_provider`
    /// table. `all_catalog_models_for_provider` falls back to that legacy table
    /// whenever the merged catalog has no rows for the provider, so this holds by
    /// construction — and it proves that the raw-legacy tail removed from the
    /// subagent `operator_model_for_subagent` consumer (which only ran when the
    /// facade was empty) was unreachable whenever legacy was non-empty. The
    /// migrated consumer is therefore behavior-preserving: it always has a
    /// catalog-sourced model to pick and never narrows to fewer choices than the
    /// legacy path offered.
    ///
    /// Note: the facade is intentionally *catalog-authoritative* (live >
    /// bundled > legacy fallback, #4188), so for some providers whose catalog
    /// supersedes stale entries in the legacy placeholder table (e.g.
    /// OpenRouter/MiniMax revisions), the facade is not a strict superset of
    /// every legacy id. That divergence does not affect subagent model
    /// *acceptance*, which is gated by `validate_route` /
    /// `requested_model_for_provider`, not by this list.
    #[test]
    fn catalog_facade_covers_every_provider_with_a_legacy_table() {
        let _env = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("temporary CODEX_HOME");
        let _codex_home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        for &provider in ApiProvider::all() {
            let legacy_len = model_completion_names_for_provider(provider).len();
            if legacy_len == 0 {
                continue;
            }
            assert!(
                !all_catalog_models_for_provider(provider).is_empty(),
                "catalog facade returned no models for {provider:?} despite a \
                 non-empty legacy table ({legacy_len} entries): the operator-route \
                 consumer would have nothing to enumerate"
            );
        }
    }

    /// #4188: CodeWhale-only / local providers keep defaults via the legacy
    /// fallback when Models.dev (live or bundled) has no rows for them.
    #[test]
    fn codewhale_only_providers_keep_legacy_defaults() {
        let _env = crate::test_support::lock_test_env();
        let codex_home = tempfile::tempdir().expect("temporary CODEX_HOME");
        let _codex_home = crate::test_support::EnvVarGuard::set("CODEX_HOME", codex_home.path());
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let openai_codex = all_catalog_models_for_provider(ApiProvider::OpenaiCodex);
        assert!(
            !openai_codex.is_empty(),
            "openai-codex must keep a default model offline: {openai_codex:?}"
        );
        assert_eq!(
            openai_codex,
            model_completion_names_for_provider(ApiProvider::OpenaiCodex)
                .iter()
                .map(|m| (*m).to_string())
                .collect::<Vec<_>>(),
            "openai-codex should come from the compatibility fallback table"
        );

        // Ollama intentionally has an empty legacy table (user-supplied ids);
        // the lake must still return empty rather than inventing rows.
        assert!(all_catalog_models_for_provider(ApiProvider::Ollama).is_empty());
        assert!(model_completion_names_for_provider(ApiProvider::Ollama).is_empty());
        assert!(live_per_provider_models(ApiProvider::Ollama).is_empty());
    }

    #[test]
    fn ollama_live_default_uses_per_provider_tags_not_models_dev() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "ollama".to_string(),
                    wire_model_id: "deepseek-v4-flash".to_string(),
                    endpoint_key: "chat".to_string(),
                    default_for_provider: true,
                    ..Default::default()
                }],
            },
            LiveSource::ModelsDev,
        );
        assert!(
            live_per_provider_models(ApiProvider::Ollama).is_empty(),
            "Models.dev must not satisfy a local Ollama default"
        );

        merge_live_offerings(vec![CatalogOffering {
            provider: "ollama".to_string(),
            wire_model_id: "qwen2.5:0.5b".to_string(),
            endpoint_key: "chat".to_string(),
            default_for_provider: true,
            ..Default::default()
        }]);
        assert_eq!(
            live_per_provider_models(ApiProvider::Ollama),
            vec!["qwen2.5:0.5b".to_string()]
        );
        clear_live_snapshot();
    }

    /// #4116 / #4188 (AC): a provider with no bundled/live catalog coverage must
    /// fall back to the legacy table verbatim, so CodeWhale-only routes stay
    /// usable. We assert this for every currently-unbundled provider that still
    /// carries a non-empty legacy list, and require at least one such provider
    /// to exist so the fallback path is actually exercised.
    #[test]
    fn unbundled_provider_falls_back_to_legacy_table() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let merged = merged_snapshot();
        let mut exercised = 0usize;
        for &provider in ApiProvider::all() {
            // OpenAI Codex deliberately owns an account-scoped cache source;
            // its fallback behavior is covered separately above.
            if provider == ApiProvider::OpenaiCodex {
                continue;
            }
            let catalog_id = catalog_provider_id(provider);
            let has_catalog_rows = !merged.offerings_for_provider(catalog_id).is_empty();
            let legacy = model_completion_names_for_provider(provider);
            if has_catalog_rows || legacy.is_empty() {
                continue;
            }
            // Unbundled + non-empty legacy: the facade must echo the legacy list.
            let facade = all_catalog_models_for_provider(provider);
            let expected: Vec<String> = legacy.iter().map(|m| m.to_string()).collect();
            assert_eq!(
                facade, expected,
                "unbundled provider {provider:?} did not fall back to the legacy table"
            );
            exercised += 1;
        }
        assert!(
            exercised > 0,
            "expected at least one unbundled provider to exercise the legacy fallback path"
        );
    }

    /// #4188: live Models.dev rows win over bundled on identity, and clearing
    /// live restores the offline bundled snapshot (offline startup still works).
    #[test]
    fn live_snapshot_merges_over_bundled() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        // With no live snapshot, we get bundled models.
        let bundled = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert!(!bundled.is_empty());

        // Set a live snapshot that adds a synthetic model.
        let live = CatalogSnapshot {
            offerings: vec![CatalogOffering {
                provider: "deepseek".to_string(),
                wire_model_id: "deepseek-v4-synthetic".to_string(),
                endpoint_key: "chat".to_string(),
                ..Default::default()
            }],
        };
        set_live_snapshot(live, LiveSource::ModelsDev);
        let merged = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert!(merged.contains(&"deepseek-v4-synthetic".to_string()));
        // The bundled model is still present.
        assert!(merged.iter().any(|m| bundled.contains(m)));

        clear_live_snapshot();
        let after_clear = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert_eq!(after_clear, bundled);
    }

    #[test]
    fn live_catalog_origin_prefers_per_provider_over_models_dev() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let wire = "accounts/fireworks/models/deepseek-v4-flash-0731";
        assert_eq!(live_catalog_origin(ApiProvider::Fireworks, wire), None);

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "fireworks".to_string(),
                    wire_model_id: wire.to_string(),
                    endpoint_key: "chat".to_string(),
                    ..Default::default()
                }],
            },
            LiveSource::ModelsDev,
        );
        assert_eq!(
            live_catalog_origin(ApiProvider::Fireworks, wire),
            Some(LiveSource::ModelsDev)
        );

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "fireworks".to_string(),
                    wire_model_id: wire.to_string(),
                    endpoint_key: "chat".to_string(),
                    ..Default::default()
                }],
            },
            LiveSource::PerProvider,
        );
        assert_eq!(
            live_catalog_origin(ApiProvider::Fireworks, wire),
            Some(LiveSource::PerProvider)
        );
        clear_live_snapshot();
    }

    /// Memoization: repeated `merged_snapshot()` calls return the cached merge
    /// (same `Arc` allocation), and publishing or clearing a live snapshot
    /// invalidates the cache so new content becomes visible.
    #[test]
    fn merged_snapshot_cache_invalidates_on_live_snapshot_change() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        let bundled_only = merged_snapshot();
        assert!(
            Arc::ptr_eq(&bundled_only, &merged_snapshot()),
            "repeated merged_snapshot() calls must return the cached Arc"
        );
        let probe = "deepseek-cache-probe-model";
        assert!(
            !bundled_only
                .offerings
                .iter()
                .any(|row| row.wire_model_id == probe),
            "probe model must not pre-exist in the bundled snapshot"
        );

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "deepseek".to_string(),
                    wire_model_id: probe.to_string(),
                    endpoint_key: "chat".to_string(),
                    ..Default::default()
                }],
            },
            LiveSource::ModelsDev,
        );
        let with_live = merged_snapshot();
        assert!(
            !Arc::ptr_eq(&bundled_only, &with_live),
            "set_live_snapshot must invalidate the memoized merge"
        );
        assert!(
            with_live
                .offerings
                .iter()
                .any(|row| row.wire_model_id == probe),
            "new live content must be visible after set_live_snapshot"
        );

        clear_live_snapshot();
        let after_clear = merged_snapshot();
        assert!(
            !after_clear
                .offerings
                .iter()
                .any(|row| row.wire_model_id == probe),
            "clear_live_snapshot must invalidate the memoized merge"
        );
        assert_eq!(
            after_clear.offerings, bundled_only.offerings,
            "clearing live must restore the bundled-only merge content"
        );
    }

    #[test]
    fn opencode_go_lake_drops_messages_only_saved_and_live_rows() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        let mut offerings: Vec<_> = crate::config::OPENCODE_GO_CHAT_MODELS
            .iter()
            .map(|model| CatalogOffering {
                provider: "opencode_go".to_string(),
                wire_model_id: if *model == crate::config::DEFAULT_OPENCODE_GO_MODEL {
                    format!("opencode-go/{model}")
                } else {
                    (*model).to_string()
                },
                endpoint_key: "chat".to_string(),
                ..Default::default()
            })
            .collect();
        offerings.extend(["minimax-m3", "qwen3.7-max"].map(|model| CatalogOffering {
            provider: "opencode-go".to_string(),
            wire_model_id: model.to_string(),
            endpoint_key: "messages".to_string(),
            ..Default::default()
        }));
        set_live_snapshot(CatalogSnapshot { offerings }, LiveSource::ModelsDev);

        let models: std::collections::BTreeSet<_> =
            all_catalog_models_for_provider(ApiProvider::OpencodeGo)
                .into_iter()
                .collect();
        let expected: std::collections::BTreeSet<_> = crate::config::OPENCODE_GO_CHAT_MODELS
            .iter()
            .map(|model| (*model).to_string())
            .collect();
        assert_eq!(models, expected);
        for messages_only in ["minimax-m3", "qwen3.7-max"] {
            assert!(
                catalog_offering_for_model(ApiProvider::OpencodeGo, messages_only).is_none(),
                "saved/live {messages_only} row must not bypass the Chat-only lake cutline"
            );
        }
        assert!(
            catalog_offering_for_model(
                ApiProvider::OpencodeGo,
                crate::config::DEFAULT_OPENCODE_GO_MODEL,
            )
            .is_some()
        );

        clear_live_snapshot();
    }

    /// #4188: live > bundled > legacy fallback precedence, including live
    /// override of a bundled wire id and no duplicate rows after alias
    /// normalization (`moonshotai` → `moonshot`).
    #[test]
    fn live_over_bundled_over_legacy_precedence_and_alias_dedupe() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        let bundled_moonshot = all_catalog_models_for_provider(ApiProvider::Moonshot);
        assert!(
            !bundled_moonshot.is_empty(),
            "offline bundled Moonshot seed required: {bundled_moonshot:?}"
        );

        // Live rows use the Models.dev alias id; lake merge must normalize onto
        // CodeWhale `moonshot` and not leave a parallel `moonshotai` bucket.
        let live = CatalogSnapshot {
            offerings: vec![
                CatalogOffering {
                    provider: "moonshot".to_string(),
                    wire_model_id: "kimi-k2.5-live".to_string(),
                    endpoint_key: "chat".to_string(),
                    default_for_provider: true,
                    ..Default::default()
                },
                // Same identity as a typical bundled Moonshot default — live wins.
                CatalogOffering {
                    provider: "moonshot".to_string(),
                    wire_model_id: bundled_moonshot[0].clone(),
                    endpoint_key: "chat".to_string(),
                    family: Some("live-override".to_string()),
                    ..Default::default()
                },
            ],
        };
        set_live_snapshot(live, LiveSource::ModelsDev);

        let merged = merged_snapshot();
        let moonshot_rows = merged.offerings_for_provider("moonshot");
        assert!(
            moonshot_rows
                .iter()
                .any(|r| r.wire_model_id == "kimi-k2.5-live"),
            "live-only Moonshot row missing: {moonshot_rows:?}"
        );
        let overridden = moonshot_rows
            .iter()
            .find(|r| r.wire_model_id == bundled_moonshot[0])
            .expect("bundled Moonshot id should still exist after live merge");
        assert_eq!(
            overridden.family.as_deref(),
            Some("live-override"),
            "live row must replace bundled facts on the same wire id"
        );
        assert!(
            merged.offerings_for_provider("moonshotai").is_empty(),
            "alias-normalized providers must not leave a duplicate moonshotai bucket"
        );

        let models = all_catalog_models_for_provider(ApiProvider::Moonshot);
        let mut seen = std::collections::BTreeSet::new();
        for model in &models {
            assert!(
                seen.insert(model.to_ascii_lowercase()),
                "duplicate Moonshot model row after alias merge: {model}"
            );
        }
        assert!(models.contains(&"kimi-k2.5-live".to_string()));

        // Legacy fallback is skipped when catalog rows exist (even if legacy
        // lists additional ids) — catalog is authoritative once non-empty.
        assert!(
            !model_completion_names_for_provider(ApiProvider::Moonshot).is_empty(),
            "legacy Moonshot table should still exist as fallback documentation"
        );

        clear_live_snapshot();
        assert_eq!(
            all_catalog_models_for_provider(ApiProvider::Moonshot),
            bundled_moonshot,
            "clearing live must restore offline bundled Moonshot rows"
        );
    }

    /// #4188: when live Models.dev emits both an alias id and the CodeWhale id
    /// for the same provider, compiling through `live_offerings_from_models_dev`
    /// then merging into the lake must not produce duplicate model rows.
    #[test]
    fn alias_normalized_live_rows_do_not_duplicate_in_lake() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();
        let body = r#"{
          "models": {},
          "providers": {
            "moonshotai": {
              "id": "moonshotai",
              "models": {
                "kimi-k2.5": {
                  "id": "kimi-k2.5",
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            },
            "moonshot": {
              "id": "moonshot",
              "models": {
                "kimi-k2.5": {
                  "id": "kimi-k2.5",
                  "modalities": { "input": ["text"], "output": ["text"] },
                  "limit": { "context": 262144, "output": 8192 }
                },
                "kimi-k2.7-code": {
                  "id": "kimi-k2.7-code",
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            }
          }
        }"#;
        let catalog =
            codewhale_config::models_dev::ModelsDevCatalog::parse_json(body).expect("parse");
        let live_rows = codewhale_config::catalog::live_offerings_from_models_dev(
            &catalog,
            "alias-fp",
            1_700_000_000,
        );
        assert!(
            live_rows.iter().all(|r| r.provider == "moonshot"),
            "both moonshotai and moonshot must normalize onto moonshot: {:?}",
            live_rows
                .iter()
                .map(|r| r.provider.as_str())
                .collect::<Vec<_>>()
        );
        set_live_snapshot(
            CatalogSnapshot {
                offerings: live_rows,
            },
            LiveSource::ModelsDev,
        );

        let models = all_catalog_models_for_provider(ApiProvider::Moonshot);
        let kimi_count = models.iter().filter(|m| m.as_str() == "kimi-k2.5").count();
        assert_eq!(
            kimi_count, 1,
            "alias-normalized providers must not duplicate kimi-k2.5: {models:?}"
        );
        assert!(
            merged_snapshot()
                .offerings_for_provider("moonshotai")
                .is_empty()
        );
        clear_live_snapshot();
    }

    // ── Source-scoped partition tests (#4188 race fix) ──────────────────────

    #[test]
    fn provider_live_snapshots_are_scoped_per_provider() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "telecomjs".to_string(),
                    wire_model_id: "deepseek-v4-pro".to_string(),
                    endpoint_key: "chat".to_string(),
                    ..Default::default()
                }],
            },
            LiveSource::PerProvider,
        );
        let telecom_only = merged_snapshot();
        assert_eq!(telecom_only.offerings_for_provider("telecomjs").len(), 1);

        set_live_snapshot(
            CatalogSnapshot {
                offerings: vec![CatalogOffering {
                    provider: "another-gateway".to_string(),
                    wire_model_id: "another-model".to_string(),
                    endpoint_key: "chat".to_string(),
                    ..Default::default()
                }],
            },
            LiveSource::PerProvider,
        );

        let merged = merged_snapshot();
        assert_eq!(merged.offerings_for_provider("telecomjs").len(), 1);
        assert_eq!(merged.offerings_for_provider("another-gateway").len(), 1);
        assert!(
            !Arc::ptr_eq(&telecom_only, &merged),
            "publishing a second provider must invalidate the cached merge"
        );

        clear_live_snapshot();
    }

    /// Models.dev→TelecomJS completion order: Models.dev sets its snapshot first,
    /// then TelecomJS merges per-provider rows. Both sets must be present in the
    /// final merged view.
    #[test]
    fn models_dev_first_then_telecomjs_both_preserved() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        // 1) Models.dev publishes its cross-provider snapshot.
        let models_dev_rows = vec![
            CatalogOffering {
                provider: "deepseek".to_string(),
                wire_model_id: "deepseek-chat".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("deepseek".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 1000,
                },
                ..Default::default()
            },
            CatalogOffering {
                provider: "zai".to_string(),
                wire_model_id: "glm-4".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("glm".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 1000,
                },
                ..Default::default()
            },
        ];
        set_live_snapshot(
            CatalogSnapshot {
                offerings: models_dev_rows,
            },
            LiveSource::ModelsDev,
        );
        let before_provider_refresh = merged_snapshot();
        assert!(
            before_provider_refresh
                .offerings_for_provider("telecomjs")
                .is_empty()
        );

        // 2) TelecomJS merges its per-provider rows (after Models.dev completes).
        let telecomjs_rows = vec![
            CatalogOffering {
                provider: "telecomjs".to_string(),
                wire_model_id: "deepseek-chat".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("deepseek".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "telecomjs-fp".to_string(),
                    fetched_at: 2000,
                },
                ..Default::default()
            },
            CatalogOffering {
                provider: "telecomjs".to_string(),
                wire_model_id: "glm-4".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("glm".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "telecomjs-fp".to_string(),
                    fetched_at: 2000,
                },
                ..Default::default()
            },
        ];
        merge_live_offerings(telecomjs_rows);
        assert_eq!(
            merged_snapshot().offerings_for_provider("telecomjs").len(),
            2,
            "provider refresh should invalidate the cached Models.dev-only view"
        );

        // 3) Both sources' rows are present in the merged snapshot.
        let merged = merged_snapshot();
        let deepseek_rows = merged.offerings_for_provider("deepseek");
        assert!(
            deepseek_rows
                .iter()
                .any(|r| r.wire_model_id == "deepseek-chat"),
            "Models.dev deepseek row missing: {deepseek_rows:?}"
        );
        let zai_rows = merged.offerings_for_provider("zai");
        assert!(
            zai_rows.iter().any(|r| r.wire_model_id == "glm-4"),
            "Models.dev zai row missing: {zai_rows:?}"
        );
        let telecomjs_rows_merged = merged.offerings_for_provider("telecomjs");
        assert_eq!(
            telecomjs_rows_merged.len(),
            2,
            "TelecomJS rows missing: {telecomjs_rows_merged:?}"
        );
        assert!(
            telecomjs_rows_merged
                .iter()
                .any(|r| r.wire_model_id == "deepseek-chat"),
            "TelecomJS deepseek-chat row missing"
        );
        assert!(
            telecomjs_rows_merged
                .iter()
                .any(|r| r.wire_model_id == "glm-4"),
            "TelecomJS glm-4 row missing"
        );

        clear_live_snapshot();
    }

    /// TelecomJS→Models.dev completion order: TelecomJS merges first, then
    /// Models.dev replaces the cross-provider snapshot. TelecomJS rows must
    /// survive the Models.dev refresh (they live in a separate partition).
    #[test]
    fn telecomjs_first_then_models_dev_both_preserved() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        // 1) TelecomJS merges its per-provider rows first.
        let telecomjs_rows = vec![
            CatalogOffering {
                provider: "telecomjs".to_string(),
                wire_model_id: "deepseek-chat".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("deepseek".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "telecomjs-fp".to_string(),
                    fetched_at: 2000,
                },
                ..Default::default()
            },
            CatalogOffering {
                provider: "telecomjs".to_string(),
                wire_model_id: "glm-4".to_string(),
                endpoint_key: "chat".to_string(),
                family: Some("glm".to_string()),
                source: CatalogSource::Live {
                    base_url_fingerprint: "telecomjs-fp".to_string(),
                    fetched_at: 2000,
                },
                ..Default::default()
            },
        ];
        merge_live_offerings(telecomjs_rows);
        assert_eq!(
            merged_snapshot().offerings_for_provider("telecomjs").len(),
            2,
            "provider rows should be visible before Models.dev completes"
        );

        // 2) Models.dev refreshes and replaces its cross-provider snapshot.
        //    Before the source-scoped fix, this would have wiped TelecomJS rows.
        let models_dev_rows = vec![CatalogOffering {
            provider: "deepseek".to_string(),
            wire_model_id: "deepseek-chat".to_string(),
            endpoint_key: "chat".to_string(),
            family: Some("deepseek".to_string()),
            source: CatalogSource::Live {
                base_url_fingerprint: "modelsdev-fp".to_string(),
                fetched_at: 3000,
            },
            ..Default::default()
        }];
        set_live_snapshot(
            CatalogSnapshot {
                offerings: models_dev_rows,
            },
            LiveSource::ModelsDev,
        );

        // 3) Both sources' rows are present — TelecomJS rows were NOT erased.
        let merged = merged_snapshot();
        let telecomjs_rows_merged = merged.offerings_for_provider("telecomjs");
        assert_eq!(
            telecomjs_rows_merged.len(),
            2,
            "TelecomJS rows were erased by Models.dev refresh: {telecomjs_rows_merged:?}"
        );
        assert!(
            telecomjs_rows_merged
                .iter()
                .any(|r| r.wire_model_id == "deepseek-chat"),
            "TelecomJS deepseek-chat row erased"
        );
        assert!(
            telecomjs_rows_merged
                .iter()
                .any(|r| r.wire_model_id == "glm-4"),
            "TelecomJS glm-4 row erased"
        );
        let deepseek_rows = merged.offerings_for_provider("deepseek");
        assert!(
            deepseek_rows
                .iter()
                .any(|r| r.wire_model_id == "deepseek-chat"),
            "Models.dev deepseek row missing: {deepseek_rows:?}"
        );

        clear_live_snapshot();
    }

    /// Catalog refresh never deletes previously published rows: a Models.dev
    /// refresh that adds new rows must preserve existing per-provider rows,
    /// and a per-provider merge must preserve existing Models.dev rows.
    #[test]
    fn catalog_refresh_never_deletes_previously_published_rows() {
        let _live = lock_live_snapshot();
        clear_live_snapshot();

        // 1) Initial state: Models.dev publishes rows for deepseek + zai.
        let initial_models_dev = vec![
            CatalogOffering {
                provider: "deepseek".to_string(),
                wire_model_id: "deepseek-chat".to_string(),
                endpoint_key: "chat".to_string(),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 1000,
                },
                ..Default::default()
            },
            CatalogOffering {
                provider: "zai".to_string(),
                wire_model_id: "glm-4".to_string(),
                endpoint_key: "chat".to_string(),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 1000,
                },
                ..Default::default()
            },
        ];
        set_live_snapshot(
            CatalogSnapshot {
                offerings: initial_models_dev,
            },
            LiveSource::ModelsDev,
        );

        // 2) TelecomJS merges its rows.
        let telecomjs_rows = vec![CatalogOffering {
            provider: "telecomjs".to_string(),
            wire_model_id: "deepseek-chat".to_string(),
            endpoint_key: "chat".to_string(),
            source: CatalogSource::Live {
                base_url_fingerprint: "telecomjs-fp".to_string(),
                fetched_at: 2000,
            },
            ..Default::default()
        }];
        merge_live_offerings(telecomjs_rows);

        // Record what we have before the second refresh.
        let before_refresh = merged_snapshot();
        let before_providers: std::collections::BTreeSet<_> = before_refresh
            .offerings
            .iter()
            .map(|r| (r.provider.clone(), r.wire_model_id.clone()))
            .collect();
        assert!(
            before_providers.contains(&("deepseek".to_string(), "deepseek-chat".to_string())),
            "deepseek row should exist before refresh"
        );
        assert!(
            before_providers.contains(&("telecomjs".to_string(), "deepseek-chat".to_string())),
            "telecomjs row should exist before refresh"
        );

        // 3) Models.dev refreshes again with an updated snapshot (adds a new row).
        let updated_models_dev = vec![
            CatalogOffering {
                provider: "deepseek".to_string(),
                wire_model_id: "deepseek-chat".to_string(),
                endpoint_key: "chat".to_string(),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 3000,
                },
                ..Default::default()
            },
            CatalogOffering {
                provider: "zai".to_string(),
                wire_model_id: "glm-4".to_string(),
                endpoint_key: "chat".to_string(),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 3000,
                },
                ..Default::default()
            },
            // New row added by the refresh.
            CatalogOffering {
                provider: "moonshot".to_string(),
                wire_model_id: "kimi-k2.5".to_string(),
                endpoint_key: "chat".to_string(),
                source: CatalogSource::Live {
                    base_url_fingerprint: "modelsdev-fp".to_string(),
                    fetched_at: 3000,
                },
                ..Default::default()
            },
        ];
        set_live_snapshot(
            CatalogSnapshot {
                offerings: updated_models_dev,
            },
            LiveSource::ModelsDev,
        );

        // 4) The TelecomJS row is STILL present — it was not deleted.
        let after_refresh = merged_snapshot();
        let after_telecomjs: Vec<_> = after_refresh
            .offerings_for_provider("telecomjs")
            .iter()
            .map(|r| r.wire_model_id.clone())
            .collect();
        assert!(
            after_telecomjs.iter().any(|id| id == "deepseek-chat"),
            "TelecomJS row was deleted by Models.dev refresh! Remaining: {after_telecomjs:?}"
        );

        // 5) New Models.dev row is also present.
        let after_moonshot: Vec<_> = after_refresh
            .offerings_for_provider("moonshot")
            .iter()
            .map(|r| r.wire_model_id.clone())
            .collect();
        assert!(
            after_moonshot.iter().any(|id| id == "kimi-k2.5"),
            "New Models.dev moonshot row missing: {after_moonshot:?}"
        );

        // 6) Also verify: a per-provider merge does not delete Models.dev rows.
        let extra_telecomjs = vec![CatalogOffering {
            provider: "telecomjs".to_string(),
            wire_model_id: "glm-4".to_string(),
            endpoint_key: "chat".to_string(),
            source: CatalogSource::Live {
                base_url_fingerprint: "telecomjs-fp".to_string(),
                fetched_at: 4000,
            },
            ..Default::default()
        }];
        merge_live_offerings(extra_telecomjs);

        let final_merged = merged_snapshot();
        let final_deepseek: Vec<_> = final_merged
            .offerings_for_provider("deepseek")
            .iter()
            .map(|r| r.wire_model_id.clone())
            .collect();
        assert!(
            final_deepseek.iter().any(|id| id == "deepseek-chat"),
            "Models.dev deepseek row was deleted by per-provider merge! Remaining: {final_deepseek:?}"
        );
        let final_moonshot: Vec<_> = final_merged
            .offerings_for_provider("moonshot")
            .iter()
            .map(|r| r.wire_model_id.clone())
            .collect();
        assert!(
            final_moonshot.iter().any(|id| id == "kimi-k2.5"),
            "Models.dev moonshot row was deleted by per-provider merge! Remaining: {final_moonshot:?}"
        );

        clear_live_snapshot();
    }
}
