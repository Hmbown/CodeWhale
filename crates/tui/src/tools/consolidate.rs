//! `consolidate` tool — manage and maintain the hippocampal memory store.
//!
//! Provides operations for memory management: merging duplicate facts,
//! rolling back to previous versions, pruning low-importance facts,
//! and reporting memory statistics. This is the maintenance counterpart
//! to `memorize` and `recall`.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, optional_u64,
};

/// Tool that manages the hippocampal memory store.
pub struct ConsolidateTool;

#[async_trait]
impl ToolSpec for ConsolidateTool {
    fn name(&self) -> &'static str {
        "consolidate"
    }

    fn description(&self) -> &'static str {
        "Manage and maintain the hippocampal memory store. \
         Supports four actions: \
         'stats' — report memory usage statistics; \
         'rollback' — restore a fact to a previous version; \
         'prune' — delete low-importance facts older than N days; \
         'merge' — consolidate duplicate facts by content. \
         Use this to keep the memory store healthy and relevant."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["stats", "rollback", "prune", "merge"],
                    "description": "Operation to perform: 'stats' (report), 'rollback' (restore version), 'prune' (delete old/low-importance), 'merge' (deduplicate)"
                },
                "fact_id": {
                    "type": "string",
                    "description": "Required for rollback: the ID of the fact to restore"
                },
                "target_version": {
                    "type": "integer",
                    "description": "Required for rollback: the version number to restore to"
                },
                "importance_threshold": {
                    "type": "number",
                    "description": "For prune: delete facts below this importance (0.0–1.0, default 0.3)"
                },
                "older_than_days": {
                    "type": "integer",
                    "description": "For prune: only delete facts older than this many days (default 0 = all ages)"
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Suggest
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::missing_field("action"))?;

        let store = context.memory_store.as_ref().ok_or_else(|| {
            ToolError::execution_failed("hippocampal memory is not available")
        })?;

        match action {
            "stats" => {
                let stats = store.get_memory_stats().map_err(|e| {
                    ToolError::execution_failed(format!("failed to get stats: {e}"))
                })?;

                let mut result = String::from("Memory Store Statistics:\n");
                result.push_str(&format!("  Entities:     {}\n", stats.total_entities));
                result.push_str(&format!("  Relations:    {}\n", stats.total_relations));
                result.push_str(&format!("  Facts:        {}\n", stats.total_facts));
                result.push_str(&format!("  Glossary:     {}\n", stats.total_glossary_terms));
                result.push_str(&format!("  Namespaces:   {}\n", stats.total_namespaces));
                result.push_str(&format!("  Avg importance: {:.2}\n", stats.avg_importance));
                if let Some(ref oldest) = stats.oldest_fact {
                    result.push_str(&format!("  Oldest fact:   {oldest}\n"));
                }
                if let Some(ref newest) = stats.newest_fact {
                    result.push_str(&format!("  Newest fact:   {newest}\n"));
                }

                Ok(ToolResult::success(result))
            }

            "rollback" => {
                let fact_id = optional_str(&input, "fact_id").ok_or_else(|| {
                    ToolError::missing_field("fact_id")
                })?;
                let target_version = input
                    .get("target_version")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| ToolError::missing_field("target_version"))?;

                let restored = store.rollback_fact(&fact_id, target_version).map_err(|e| {
                    ToolError::execution_failed(format!("rollback failed: {e}"))
                })?;

                Ok(ToolResult::success(format!(
                    "Rolled back fact to version {} (now at v{}):\n{}",
                    target_version, restored.version, restored.content
                )))
            }

            "prune" => {
                let threshold = input
                    .get("importance_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.3)
                    .clamp(0.0, 1.0);
                let older_than_days = input
                    .get("older_than_days")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                let count = store
                    .prune_low_importance_facts(threshold, older_than_days)
                    .map_err(|e| ToolError::execution_failed(format!("prune failed: {e}")))?;

                if older_than_days > 0 {
                    Ok(ToolResult::success(format!(
                        "Pruned {count} facts with importance < {threshold:.1} older than {older_than_days} days."
                    )))
                } else {
                    Ok(ToolResult::success(format!(
                        "Pruned {count} facts with importance < {threshold:.1}."
                    )))
                }
            }

            "merge" => {
                // Deduplicate: group facts by similar content, keep the one with highest importance
                let all_facts = store.important_facts(1000).map_err(|e| {
                    ToolError::execution_failed(format!("failed to fetch facts: {e}"))
                })?;

                // Simple exact-content dedup (semantic merge deferred to a follow-up)
                use std::collections::HashMap;
                let mut seen: HashMap<String, (String, f64)> = HashMap::new(); // canon_key -> (id, importance)
                let mut merged = 0usize;

                for fact in &all_facts {
                    let canon_key = fact.content.trim().to_lowercase();
                    if let Some((existing_id, existing_imp)) = seen.get(&canon_key) {
                        if fact.importance > *existing_imp {
                            // Current fact is more important, keep it instead
                            let _ = store.delete_fact(existing_id);
                            seen.insert(canon_key, (fact.id.clone(), fact.importance));
                        } else {
                            let _ = store.delete_fact(&fact.id);
                        }
                        merged += 1;
                    } else {
                        seen.insert(canon_key, (fact.id.clone(), fact.importance));
                    }
                }

                Ok(ToolResult::success(format!("Merged and removed {merged} duplicate facts.")))
            }

            _ => Err(ToolError::execution_failed(format!(
                "Unknown action '{action}'. Use 'stats', 'rollback', 'prune', or 'merge'."
            ))),
        }
    }
}
