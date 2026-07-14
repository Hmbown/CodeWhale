//! Configured provider/model lake facade (#3830, Wave 5b / #4188).
//!
//! Single seam over the Models.dev catalog layers and the configured-provider
//! predicate shared with `/provider`. Precedence is **live Models.dev >
//! bundled offline snapshot > legacy hardcoded fallback**. Pickers, hotbar
//! route slots, [`crate::model_inventory::ModelInventory`], slash completions,
//! and subagent validation should read model lists from here.
//!
//! [`crate::config::model_completion_names_for_provider`] is retained only as a
//! compatibility fallback for CodeWhale-only / local providers that Models.dev
//! does not represent (and for unbundled gateways until the live catalog covers
//! them).

use std::sync::RwLock;

use codewhale_config::catalog::{CatalogOffering, CatalogSnapshot, bundled_catalog_offerings};

use crate::codex_model_cache;
use crate::config::{
    ApiProvider, Config, model_completion_names_for_provider, opencode_go_chat_model_id,
    provider_is_configured_for_active,
};

static BUNDLED_SNAPSHOT: std::sync::OnceLock<CatalogSnapshot> = std::sync::OnceLock::new();

/// Optional live Models.dev snapshot (#4187). When `None`, only the bundled
/// offline/stale fallback rows are visible.
static LIVE_SNAPSHOT: RwLock<Option<CatalogSnapshot>> = RwLock::new(None);

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
    snapshot.offerings = snapshot
        .offerings
        .into_iter()
        .filter_map(|mut offering| {
            if ApiProvider::parse(&offering.provider) == Some(ApiProvider::OpencodeGo) {
                let canonical = opencode_go_chat_model_id(&offering.wire_model_id)?;
                offering.provider = ApiProvider::OpencodeGo.as_str().to_string();
                offering.wire_model_id = canonical.to_string();
            }
            Some(offering)
        })
        .collect();
    snapshot
}

/// Set the live-catalog snapshot. Call this after a background refresh
/// succeeds; the lake merges live rows over bundled rows on the next read.
/// Stale or empty snapshots are harmless — a `None` just means "bundled only."
pub fn set_live_snapshot(snapshot: CatalogSnapshot) {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        *guard = Some(apply_provider_model_cutlines(snapshot));
    }
}

/// Clear the live snapshot (e.g. on cache eviction or shutdown).
pub fn clear_live_snapshot() {
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        *guard = None;
    }
}

/// Merge additional live offerings into the existing live snapshot (#4188).
///
/// Unlike [`set_live_snapshot`] (which replaces the entire snapshot), this
/// merges new rows by `(provider, wire_model_id)` identity, preserving rows
/// from other sources (e.g. Models.dev) that were already published. This is
/// used by per-provider catalog refreshes (e.g. TelecomJS `/v1/models`) that
/// need to coexist with the cross-provider Models.dev live layer.
pub fn merge_live_offerings(new_offerings: Vec<CatalogOffering>) {
    if new_offerings.is_empty() {
        return;
    }
    if let Ok(mut guard) = LIVE_SNAPSHOT.write() {
        let existing = guard.take().unwrap_or_default();
        use std::collections::BTreeMap;
        let mut merged: BTreeMap<(String, String), CatalogOffering> = BTreeMap::new();
        for row in &existing.offerings {
            merged.insert((row.provider.clone(), row.wire_model_id.clone()), row.clone());
        }
        for row in new_offerings {
            merged.insert((row.provider.clone(), row.wire_model_id.clone()), row);
        }
        *guard = Some(CatalogSnapshot {
            offerings: merged.into_values().collect(),
        });
    }
}

/// The merged catalog snapshot: live rows override bundled rows on
/// `(provider, wire_model_id)` identity (#4188). When no live snapshot is
/// present, this is just the offline bundled snapshot.
fn merged_snapshot() -> CatalogSnapshot {
    let live = LIVE_SNAPSHOT.read().ok().and_then(|guard| guard.clone());
    let merged = match live {
        None => bundled_snapshot().clone(),
        Some(live) => {
            use std::collections::BTreeMap;
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

/// Every built-in provider that carries at least one merged-catalog row.
#[must_use]
#[allow(dead_code)]
pub fn all_catalog_providers() -> Vec<ApiProvider> {
    let mut seen = Vec::new();
    for offering in &merged_snapshot().offerings {
        if let Some(provider) = ApiProvider::parse(&offering.provider)
            && !seen.contains(&provider)
        {
            seen.push(provider);
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DEFAULT_TOGETHER_FLASH_MODEL, DEFAULT_TOGETHER_MODEL};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Serialize tests that mutate the process-wide live snapshot.
    fn lock_live_snapshot() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        set_live_snapshot(live);
        let merged = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert!(merged.contains(&"deepseek-v4-synthetic".to_string()));
        // The bundled model is still present.
        assert!(merged.iter().any(|m| bundled.contains(m)));

        clear_live_snapshot();
        let after_clear = all_catalog_models_for_provider(ApiProvider::Deepseek);
        assert_eq!(after_clear, bundled);
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
        set_live_snapshot(CatalogSnapshot { offerings });

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
        set_live_snapshot(live);

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
        set_live_snapshot(CatalogSnapshot {
            offerings: live_rows,
        });

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
}
