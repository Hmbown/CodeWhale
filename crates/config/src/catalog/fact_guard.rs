//! Data-driven guards over bundled model/provider fact assets.
//!
//! These checks are the FIXLIST 0.5 harness: one pass over the committed
//! snapshots that would have caught output-above-window typos, split facts,
//! two-target aliases, DEFAULT_*_MODEL rows with no catalog facts, and a
//! provider-wide constant shadowing a per-model window.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

use super::{BUNDLED_MODELS_DEV_JSON, bundled_limit_facts, bundled_output_exceeds_context};
use crate::ProviderKind;
use crate::provider::all_providers;

/// TUI offline catalog snapshot. Same bytes `model_catalog` embeds.
pub const TUI_MODEL_CATALOG_JSON: &str =
    include_str!("../../../tui/assets/model_catalog.bundled.json");

/// Family-wide context fallbacks that must not overwrite a more specific
/// per-model window stored in another bundled asset.
///
/// These match the provider-wide heuristics in `crates/tui/src/models.rs`
/// (`contains("claude")` → 200K, non-V4 DeepSeek → 128K).
const FAMILY_WIDE_CONTEXT_FALLBACKS: &[(&str, u64)] = &[("claude", 200_000), ("deepseek", 128_000)];

/// A `DEFAULT_*_MODEL` (or provider `default_model()`) the guard should find
/// in a bundled provider catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultModelSpec {
    /// Canonical provider id (`openai`, `moonshot`, …).
    pub provider: String,
    /// Wire / catalog model id.
    pub model: String,
}

/// Audit one or more Models.dev / TUI catalog JSON values.
///
/// `defaults` are checked only against assets that actually contain that
/// provider's catalog. Provider-registry alias uniqueness is *not* included
/// here so a corrupt fixture can exercise the JSON rules in isolation.
#[must_use]
pub fn audit_model_provider_facts(
    assets: &[(&str, &Value)],
    defaults: &[DefaultModelSpec],
) -> Vec<String> {
    let mut violations = BTreeSet::new();

    for (name, value) in assets {
        for row in bundled_output_exceeds_context(value) {
            violations.insert(format!("{name}: {row}"));
        }
        for row in provider_wide_shadows(value) {
            violations.insert(format!("{name}: {row}"));
        }
    }

    for row in split_facts_across_assets(assets) {
        violations.insert(row);
    }
    for row in split_aliases_across_assets(assets) {
        violations.insert(row);
    }
    for row in family_wide_fallback_shadows_per_model_fact(assets) {
        violations.insert(row);
    }

    let honesty = assets
        .iter()
        .map(|(_, value)| meta_blob(value))
        .collect::<Vec<_>>()
        .join("\n");
    for spec in defaults {
        for row in default_model_violations(assets, spec, &honesty) {
            violations.insert(row);
        }
    }

    violations.into_iter().collect()
}

/// Audit the committed Models.dev + TUI catalog snapshots plus live
/// `DEFAULT_*_MODEL` / provider-alias tables.
#[must_use]
pub fn audit_committed_bundled_assets() -> Vec<String> {
    let models_dev: Value =
        serde_json::from_str(BUNDLED_MODELS_DEV_JSON).expect("bundled Models.dev JSON");
    let tui_catalog: Value =
        serde_json::from_str(TUI_MODEL_CATALOG_JSON).expect("bundled TUI catalog JSON");
    let assets = [("models_dev", &models_dev), ("tui_catalog", &tui_catalog)];
    let mut violations = audit_model_provider_facts(&assets, &committed_default_specs(&models_dev));
    violations.extend(provider_alias_collisions());
    violations
}

fn committed_default_specs(models_dev: &Value) -> Vec<DefaultModelSpec> {
    let bundled_providers = provider_ids(models_dev);
    let mut specs = Vec::new();
    for provider in all_providers() {
        if provider.kind() == ProviderKind::Custom {
            continue;
        }
        if !bundled_providers
            .iter()
            .any(|id| id.eq_ignore_ascii_case(provider.id()))
        {
            continue;
        }
        specs.push(DefaultModelSpec {
            provider: provider.id().to_string(),
            model: provider.default_model().to_string(),
        });
    }
    for (provider, model) in [
        ("openrouter", crate::DEFAULT_OPENROUTER_FLASH_MODEL),
        ("novita", crate::DEFAULT_NOVITA_FLASH_MODEL),
        ("siliconflow", crate::DEFAULT_SILICONFLOW_FLASH_MODEL),
        ("together", crate::DEFAULT_TOGETHER_FLASH_MODEL),
    ] {
        if bundled_providers
            .iter()
            .any(|id| id.eq_ignore_ascii_case(provider))
        {
            specs.push(DefaultModelSpec {
                provider: provider.to_string(),
                model: model.to_string(),
            });
        }
    }
    specs
}

fn provider_ids(value: &Value) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        return ids;
    };
    for (key, row) in providers {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(key);
        ids.insert(id.to_string());
    }
    ids
}

fn meta_blob(value: &Value) -> String {
    let Some(meta) = value.get("_meta").and_then(Value::as_object) else {
        return String::new();
    };
    let mut parts = Vec::new();
    for key in [
        "honesty",
        "pending_release_metadata",
        "about",
        "currency_sweep_2026_08_17",
        "currency_sweep_2026_08_25",
    ] {
        if let Some(text) = meta.get(key).and_then(Value::as_str) {
            parts.push(text);
        }
    }
    parts.join("\n")
}

fn provider_wide_shadows(value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(providers) = value.get("providers").and_then(Value::as_object) else {
        return out;
    };
    for (provider, row) in providers {
        let wide = fact_view(row);
        if wide.is_empty() {
            continue;
        }
        let Some(models) = row.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (key, model) in models {
            let stated = fact_view(model);
            if let (Some(wide_ctx), Some(model_ctx)) = (wide.context, stated.context)
                && wide_ctx != model_ctx
            {
                out.push(format!(
                    "providers.{provider} context {wide_ctx} shadows {key} per-model context {model_ctx}"
                ));
            }
            if let (Some(wide_out), Some(model_out)) = (wide.output, stated.output)
                && wide_out != model_out
            {
                out.push(format!(
                    "providers.{provider} output {wide_out} shadows {key} per-model output {model_out}"
                ));
            }
            if let (Some(wide_in), Some(model_in)) = (&wide.input_price, &stated.input_price)
                && wide_in != model_in
            {
                out.push(format!(
                    "providers.{provider} input price {wide_in} shadows {key} per-model price {model_in}"
                ));
            }
            if let (Some(wide_out_price), Some(model_out_price)) =
                (&wide.output_price, &stated.output_price)
                && wide_out_price != model_out_price
            {
                out.push(format!(
                    "providers.{provider} output price {wide_out_price} shadows {key} per-model price {model_out_price}"
                ));
            }
        }
    }
    out
}

#[derive(Clone, Default)]
struct FactView {
    context: Option<u64>,
    output: Option<u64>,
    input_price: Option<String>,
    output_price: Option<String>,
}

impl FactView {
    fn is_empty(&self) -> bool {
        self.context.is_none()
            && self.output.is_none()
            && self.input_price.is_none()
            && self.output_price.is_none()
    }
}

fn fact_view(row: &Value) -> FactView {
    let limit = row.get("limit");
    FactView {
        context: json_u64(limit.and_then(|limit| limit.get("context")))
            .or_else(|| json_u64(row.get("context_window")))
            .or_else(|| json_u64(row.get("context"))),
        output: json_u64(limit.and_then(|limit| limit.get("output")))
            .or_else(|| json_u64(row.get("max_output")))
            .or_else(|| json_u64(row.get("output"))),
        input_price: json_price(
            row.get("cost")
                .and_then(|cost| cost.get("input"))
                .or_else(|| row.get("input_usd_per_million")),
        ),
        output_price: json_price(
            row.get("cost")
                .and_then(|cost| cost.get("output"))
                .or_else(|| row.get("output_usd_per_million")),
        ),
    }
}

fn json_u64(value: Option<&Value>) -> Option<u64> {
    let value = value?;
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| {
            value.as_f64().and_then(|n| {
                if n.is_finite() && n >= 0.0 && n.fract() == 0.0 {
                    Some(n as u64)
                } else {
                    None
                }
            })
        })
}

fn json_price(value: Option<&Value>) -> Option<String> {
    let value = value?;
    if let Some(n) = value.as_f64()
        && n.is_finite()
    {
        return Some(format!("{n:.8}"));
    }
    value.as_u64().map(|n| format!("{n:.8}"))
}

#[derive(Clone, Default)]
struct AssetFacts {
    context: BTreeSet<u64>,
    output: BTreeSet<u64>,
    input_price: BTreeSet<String>,
    output_price: BTreeSet<String>,
}

fn facts_by_model(value: &Value) -> BTreeMap<String, AssetFacts> {
    let mut map: BTreeMap<String, AssetFacts> = BTreeMap::new();
    for fact in bundled_limit_facts(value) {
        if fact.model_id.is_empty() {
            continue;
        }
        let entry = map.entry(normalize_id(&fact.model_id)).or_default();
        if let Some(context) = fact.context {
            entry.context.insert(context);
        }
        if let Some(output) = fact.output {
            entry.output.insert(output);
        }
    }
    collect_prices_into(value, &mut map);
    map
}

fn collect_prices_into(value: &Value, map: &mut BTreeMap<String, AssetFacts>) {
    visit_model_rows(value, |id, row| {
        let facts = fact_view(row);
        let entry = map.entry(normalize_id(id)).or_default();
        if let Some(price) = facts.input_price {
            entry.input_price.insert(price);
        }
        if let Some(price) = facts.output_price {
            entry.output_price.insert(price);
        }
    });
}

fn split_facts_across_assets(assets: &[(&str, &Value)]) -> Vec<String> {
    let per_asset: Vec<(&str, BTreeMap<String, AssetFacts>)> = assets
        .iter()
        .map(|(name, value)| (*name, facts_by_model(value)))
        .collect();
    let mut ids = BTreeSet::new();
    for (_, facts) in &per_asset {
        ids.extend(facts.keys().cloned());
    }

    let mut out = Vec::new();
    for id in ids {
        let present: Vec<(&str, &AssetFacts)> = per_asset
            .iter()
            .filter_map(|(name, facts)| facts.get(&id).map(|row| (*name, row)))
            .collect();
        if present.len() < 2 {
            continue;
        }
        compare_dimension(
            &mut out,
            &id,
            "context",
            present
                .iter()
                .map(|(name, facts)| (*name, &facts.context))
                .collect(),
        );
        compare_dimension(
            &mut out,
            &id,
            "output",
            present
                .iter()
                .map(|(name, facts)| (*name, &facts.output))
                .collect(),
        );
        compare_dimension(
            &mut out,
            &id,
            "input price",
            present
                .iter()
                .map(|(name, facts)| (*name, &facts.input_price))
                .collect(),
        );
        compare_dimension(
            &mut out,
            &id,
            "output price",
            present
                .iter()
                .map(|(name, facts)| (*name, &facts.output_price))
                .collect(),
        );
    }
    out
}

fn compare_dimension<T: std::fmt::Display + Ord>(
    out: &mut Vec<String>,
    id: &str,
    dimension: &str,
    values: Vec<(&str, &BTreeSet<T>)>,
) {
    let unique: Vec<(&str, &T)> = values
        .into_iter()
        .filter(|(_, set)| set.len() == 1)
        .map(|(name, set)| (name, set.iter().next().expect("len 1")))
        .collect();
    if unique.len() < 2 {
        return;
    }
    let first = unique[0].1;
    if unique.iter().all(|(_, value)| *value == first) {
        return;
    }
    let detail = unique
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join(", ");
    out.push(format!(
        "{id} {dimension} disagrees across bundled assets ({detail})"
    ));
}

fn family_wide_fallback_shadows_per_model_fact(assets: &[(&str, &Value)]) -> Vec<String> {
    let mut out = Vec::new();
    for row in split_facts_across_assets(assets) {
        // split_facts already reports the disagreement; add a sharper label
        // when one of the values is a known provider-wide family fallback.
        for (family, fallback) in FAMILY_WIDE_CONTEXT_FALLBACKS {
            if row.contains(" context disagrees ")
                && row.contains(&format!("={fallback}"))
                && row.to_ascii_lowercase().contains(family)
            {
                out.push(format!(
                    "provider-wide {family} {fallback} fallback shadows a per-model context: {row}"
                ));
            }
        }
    }
    out
}

fn split_aliases_across_assets(assets: &[(&str, &Value)]) -> Vec<String> {
    let mut by_alias: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_name, value) in assets {
        for (alias, canonical) in collect_model_aliases(value) {
            by_alias
                .entry(normalize_id(&alias))
                .or_default()
                .insert(normalize_id(&canonical));
        }
    }
    by_alias
        .into_iter()
        .filter_map(|(alias, canons)| {
            (canons.len() > 1).then(|| {
                format!(
                    "alias `{alias}` resolves to multiple canonical ids: {}",
                    canons.into_iter().collect::<Vec<_>>().join(", ")
                )
            })
        })
        .collect()
}

fn collect_model_aliases(value: &Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    visit_model_rows(value, |id, row| {
        if let Some(canonical) = row
            .get("provider_model_id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|canonical| !canonical.is_empty() && !canonical.eq_ignore_ascii_case(id))
        {
            out.push((id.to_string(), canonical.to_string()));
        }
        if let Some(aliases) = row.get("aliases").and_then(Value::as_array) {
            for alias in aliases {
                if let Some(alias) = alias
                    .as_str()
                    .map(str::trim)
                    .filter(|alias| !alias.is_empty())
                {
                    out.push((alias.to_string(), id.to_string()));
                }
            }
        }
    });
    out
}

fn visit_model_rows(value: &Value, mut visit: impl FnMut(&str, &Value)) {
    if let Some(models) = value.get("models").and_then(Value::as_object) {
        for (key, row) in models {
            visit(&row_id(key, row), row);
        }
    }
    if let Some(providers) = value.get("providers").and_then(Value::as_object) {
        for (_provider, row) in providers {
            let Some(models) = row.get("models").and_then(Value::as_object) else {
                continue;
            };
            for (key, model) in models {
                visit(&row_id(key, model), model);
            }
        }
    }
    if let Some(entries) = value.get("entries").and_then(Value::as_object) {
        for (key, row) in entries {
            visit(&row_id(key, row), row);
        }
    }
}

fn row_id(key: &str, row: &Value) -> String {
    row.get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(key)
        .to_string()
}

fn default_model_violations(
    assets: &[(&str, &Value)],
    spec: &DefaultModelSpec,
    honesty: &str,
) -> Vec<String> {
    let mut out = Vec::new();
    let mut saw_provider = false;
    for (asset_name, value) in assets {
        let Some(models) = provider_models(value, &spec.provider) else {
            continue;
        };
        saw_provider = true;
        let Some((_key, row)) = find_model_row(models, &spec.model) else {
            out.push(format!(
                "{}/{} ({asset_name}): DEFAULT model has no catalog row",
                spec.provider, spec.model
            ));
            continue;
        };
        let facts = fact_view(row);
        if facts.context.is_none() {
            out.push(format!(
                "{}/{} ({asset_name}): DEFAULT model has no context window",
                spec.provider, spec.model
            ));
        }
        let priced = facts.input_price.is_some()
            || facts.output_price.is_some()
            || tui_entry_is_priced(assets, &spec.model);
        if !priced && !honesty_covers(honesty, &spec.provider, &spec.model) {
            out.push(format!(
                "{}/{} ({asset_name}): DEFAULT model has no price and is not documented as honestly unpriced",
                spec.provider, spec.model
            ));
        }
    }
    if !saw_provider {
        // Provider is not in the bundled seed (local/runtime/placeholder).
        // The 0.5 harness is over bundled assets, so this is not a miss.
    }
    out
}

fn provider_models<'a>(value: &'a Value, provider: &str) -> Option<&'a Map<String, Value>> {
    let providers = value.get("providers")?.as_object()?;
    for (key, row) in providers {
        let id = row
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(key);
        if id.eq_ignore_ascii_case(provider) {
            return row.get("models")?.as_object();
        }
    }
    None
}

fn find_model_row<'a>(models: &'a Map<String, Value>, model: &str) -> Option<(&'a str, &'a Value)> {
    let needle = model.trim();
    models
        .get_key_value(needle)
        .map(|(k, v)| (k.as_str(), v))
        .or_else(|| {
            models.iter().find_map(|(key, row)| {
                let id = row_id(key, row);
                (key.eq_ignore_ascii_case(needle) || id.eq_ignore_ascii_case(needle))
                    .then_some((key.as_str(), row))
            })
        })
}

fn tui_entry_is_priced(assets: &[(&str, &Value)], model: &str) -> bool {
    assets.iter().any(|(_, value)| {
        let Some(entries) = value.get("entries").and_then(Value::as_object) else {
            return false;
        };
        entries.iter().any(|(key, row)| {
            let id = row_id(key, row);
            if !key.eq_ignore_ascii_case(model) && !id.eq_ignore_ascii_case(model) {
                return false;
            }
            json_price(row.get("input_usd_per_million")).is_some()
                || json_price(row.get("output_usd_per_million")).is_some()
        })
    })
}

fn honesty_covers(honesty: &str, provider: &str, model: &str) -> bool {
    let blob = fold_id(honesty);
    let provider = fold_id(provider);
    let model = fold_id(model);
    if blob.contains(&model) {
        return true;
    }
    if provider == "deepseek" && blob.contains("deepseek-native") {
        return true;
    }
    if model.contains("deepseek") && blob.contains("aggregator-hosted-deepseek") {
        return true;
    }
    if provider == "xai" && blob.contains("grok") {
        return true;
    }
    if provider.contains("xiaomi") && blob.contains("xiaomi") {
        return true;
    }
    if provider.contains("modelstudio")
        && (blob.contains("model-studio") || blob.contains("alibaba"))
    {
        return true;
    }
    if model.contains("glm-5-3") && blob.contains("glm-5-3") {
        return true;
    }
    if model.contains("minimax-m3") && blob.contains("minimax-m3") {
        return true;
    }
    false
}

fn provider_alias_collisions() -> Vec<String> {
    let mut owners: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for provider in all_providers() {
        owners
            .entry(normalize_id(provider.id()))
            .or_default()
            .insert(provider.id().to_string());
        for alias in provider.aliases() {
            owners
                .entry(normalize_id(alias))
                .or_default()
                .insert(provider.id().to_string());
        }
    }
    owners
        .into_iter()
        .filter_map(|(alias, ids)| {
            if ids.len() <= 1 {
                return None;
            }
            // Dialect collapse: the alias *is* a second provider's canonical
            // id (deepseek-anthropic, minimax-anthropic). Parse still returns
            // exactly one catalog kind.
            if ids.iter().any(|id| normalize_id(id) == alias) {
                return None;
            }
            Some(format!(
                "provider alias `{alias}` resolves to multiple canonical ids: {}",
                ids.into_iter().collect::<Vec<_>>().join(", ")
            ))
        })
        .collect()
}

fn normalize_id(value: &str) -> String {
    fold_id(value)
}

fn fold_id(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|ch| match ch {
            '_' | ' ' | '.' => '-',
            _ => ch.to_ascii_lowercase(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tiny_asset() -> Value {
        json!({
            "providers": {
                "acme": {
                    "id": "acme",
                    "models": {
                        "acme-1": {
                            "id": "acme-1",
                            "limit": { "context": 8192, "output": 4096 },
                            "cost": { "input": 1.0, "output": 2.0 }
                        }
                    }
                }
            }
        })
    }

    #[test]
    fn corrupted_output_above_window_is_reported() {
        let mut bad = tiny_asset();
        bad["providers"]["acme"]["models"]["acme-1"]["limit"]["output"] = json!(9000);
        let hits = audit_model_provider_facts(
            &[("fixture", &bad)],
            &[DefaultModelSpec {
                provider: "acme".into(),
                model: "acme-1".into(),
            }],
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("output") && row.contains("acme-1")),
            "{hits:?}"
        );
    }

    #[test]
    fn split_facts_across_two_assets_are_reported() {
        let a = tiny_asset();
        let mut b = tiny_asset();
        b["providers"]["acme"]["models"]["acme-1"]["limit"]["context"] = json!(4096);
        let hits = audit_model_provider_facts(&[("a", &a), ("b", &b)], &[]);
        assert!(
            hits.iter().any(|row| row.contains("context disagrees")),
            "{hits:?}"
        );
    }

    #[test]
    fn corrupted_row_fails_every_fact_guard_assertion() {
        let models_dev = json!({
            "providers": {
                "fixture": {
                    "id": "fixture",
                    "limit": { "context": 128000, "output": 4096 },
                    "models": {
                        "corrupt-default": {
                            "id": "corrupt-default",
                            "default": true,
                            "limit": { "context": 1050000, "output": 2000000 },
                            "aliases": ["shared-alias"]
                        },
                        "other": {
                            "id": "other",
                            "limit": { "context": 8192, "output": 1024 },
                            "aliases": ["shared-alias"]
                        }
                    }
                }
            }
        });
        let tui_catalog = json!({
            "entries": {
                "corrupt-default": {
                    "id": "corrupt-default",
                    "context_window": 128000,
                    "max_output": 4096
                },
                "alias-owner": {
                    "id": "alias-owner",
                    "aliases": ["shared-alias"]
                }
            }
        });
        let hits = audit_model_provider_facts(
            &[("models_dev", &models_dev), ("tui_catalog", &tui_catalog)],
            &[
                DefaultModelSpec {
                    provider: "fixture".into(),
                    model: "corrupt-default".into(),
                },
                DefaultModelSpec {
                    provider: "fixture".into(),
                    model: "missing-default".into(),
                },
            ],
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("output") && row.contains("context")),
            "max_output <= context_window: {hits:?}"
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("disagrees across bundled assets")),
            "split facts: {hits:?}"
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("alias") && row.contains("multiple canonical")),
            "alias uniqueness: {hits:?}"
        );
        assert!(
            hits.iter().any(|row| row.contains("shadows")),
            "provider-wide shadowing: {hits:?}"
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("DEFAULT model has no price")),
            "DEFAULT price: {hits:?}"
        );
        assert!(
            hits.iter()
                .any(|row| row.contains("missing-default") && row.contains("no catalog row")),
            "DEFAULT catalog row: {hits:?}"
        );
    }

    #[test]
    fn committed_bundled_assets_pass_or_name_every_violation() {
        let hits = audit_committed_bundled_assets();
        assert!(
            hits.is_empty(),
            "bundled fact guard failed:\n{}",
            hits.join("\n")
        );
    }
}
