//! Apply cloud model patches to a catalog layer map (layer 15: above bundled
//! and live models.dev, below provider `/v1/models`, config, and user rows).
//!
//! Patch semantics:
//! - `Upsert`: only the fields the patch sets shadow the row; a patch for a
//!   row that does not exist is materialized only when it carries a context
//!   window (otherwise skipped with a receipt).
//! - `Deprecate`: annotates (the note is carried in `reasoning_options` as a
//!   `{"cloud_facts": {...}}` marker); never removes.
//! - `Hide`: removes the row only when it came from the bundled or live
//!   models.dev layers. Provider-live/config/user rows are never hidden.

use std::collections::BTreeMap;

use serde_json::json;

use super::scope::ScopedFacts;
use super::types::{ModelFact, ModelOp};
use crate::catalog::{CatalogOffering, CatalogSource};
use crate::models_dev::ModelsDevCost;

/// Merge key used by the catalog compiler.
type Key = (String, String);

/// Receipt for one patch that changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedPatch {
    pub provider: String,
    pub id: String,
    pub reason: String,
}

/// Apply every model patch in `facts` to `rows`, returning skip receipts.
pub fn apply_model_patches(
    rows: &mut BTreeMap<Key, CatalogOffering>,
    facts: &ScopedFacts,
    fetched_at: u64,
) -> Vec<SkippedPatch> {
    let mut skipped = Vec::new();
    let source = CatalogSource::CloudFacts {
        facts_version: facts.facts_version,
        key_id: facts.key_id.clone(),
        fetched_at,
    };
    for patch in &facts.models {
        let key = (patch.provider.clone(), patch.id.clone());
        match patch.op {
            ModelOp::Hide => match rows.get(&key) {
                Some(row)
                    if matches!(
                        row.source,
                        CatalogSource::Bundled | CatalogSource::ModelsDevLive { .. }
                    ) =>
                {
                    rows.remove(&key);
                }
                Some(_) => skipped.push(SkippedPatch {
                    provider: patch.provider.clone(),
                    id: patch.id.clone(),
                    reason: "hide ignored: row comes from a higher layer".into(),
                }),
                None => skipped.push(SkippedPatch {
                    provider: patch.provider.clone(),
                    id: patch.id.clone(),
                    reason: "hide ignored: no such row".into(),
                }),
            },
            ModelOp::Deprecate => match rows.get_mut(&key) {
                Some(row) => {
                    annotate(row, patch, "deprecated");
                }
                None => skipped.push(SkippedPatch {
                    provider: patch.provider.clone(),
                    id: patch.id.clone(),
                    reason: "deprecate ignored: no such row".into(),
                }),
            },
            ModelOp::Upsert => {
                if let Some(row) = rows.get_mut(&key) {
                    patch_fields(row, patch);
                    row.source = source.clone();
                } else if patch.context_window.is_some() {
                    let mut row = CatalogOffering {
                        provider: patch.provider.clone(),
                        wire_model_id: patch.id.clone(),
                        endpoint_key: "chat".to_string(),
                        source: source.clone(),
                        ..CatalogOffering::default()
                    };
                    patch_fields(&mut row, patch);
                    rows.insert(key, row);
                } else {
                    skipped.push(SkippedPatch {
                        provider: patch.provider.clone(),
                        id: patch.id.clone(),
                        reason: "upsert ignored: new row needs context_window".into(),
                    });
                }
            }
        }
    }
    skipped
}

fn patch_fields(row: &mut CatalogOffering, patch: &ModelFact) {
    if patch.context_window.is_some() || patch.max_output.is_some() {
        let mut limit = row.limit.clone().unwrap_or_default();
        if let Some(context) = patch.context_window {
            limit.context = Some(context);
        }
        if let Some(output) = patch.max_output {
            limit.output = Some(output);
        }
        row.limit = Some(limit);
    }
    if let Some(pricing) = &patch.pricing {
        let mut cost: ModelsDevCost = row.cost.clone().unwrap_or_default();
        if pricing.input_per_m.is_some() {
            cost.input = pricing.input_per_m;
        }
        if pricing.output_per_m.is_some() {
            cost.output = pricing.output_per_m;
        }
        if pricing.cache_read_per_m.is_some() {
            cost.cache_read = pricing.cache_read_per_m;
        }
        row.cost = Some(cost);
    }
    if patch.reasoning.is_some() {
        row.reasoning = patch.reasoning;
    }
    if patch.display_name.is_some() || patch.note.is_some() {
        annotate(row, patch, "upsert");
    }
}

fn annotate(row: &mut CatalogOffering, patch: &ModelFact, kind: &str) {
    row.reasoning_options
        .retain(|value| value.get("cloud_facts").is_none());
    row.reasoning_options.push(json!({
        "cloud_facts": {
            "op": kind,
            "display_name": patch.display_name,
            "deprecated_at": patch.deprecated_at,
            "replacement": patch.replacement,
            "note": patch.note,
        }
    }));
}
