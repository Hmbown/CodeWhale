//! `memorize` tool — store a structured fact in the hippocampal memory store.
//!
//! Unlike the simpler `remember` tool (which appends a bullet to `memory.md`),
//! `memorize` records a fact in the SQLite-backed entity graph with importance
//! scoring and optional entity binding. Facts stored here survive compaction
//! and can be recalled across sessions via the `recall` tool.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};

/// Tool that records a structured fact in the hippocampal memory store.
pub struct MemorizeTool;

#[async_trait]
impl ToolSpec for MemorizeTool {
    fn name(&self) -> &'static str {
        "memorize"
    }

    fn description(&self) -> &'static str {
        "Store a structured fact in long-term memory. Facts survive compaction and \
         can be recalled across sessions. Use this when you learn something important \
         about the project, the user's preferences, architecture decisions, or anything \
         you should remember later. Optionally associate the fact with an entity \
         (file, issue, person) for graph-based recall. High-importance facts (0.8+) \
         are retained indefinitely; low-importance facts may be pruned over time."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "The factual statement to remember."
                },
                "entity_kind": {
                    "type": "string",
                    "description": "Optional entity type: 'file', 'issue', 'pr', 'concept', 'decision', 'person', 'config'"
                },
                "entity_name": {
                    "type": "string",
                    "description": "Optional entity name (e.g. 'dispatch.rs', 'PR #2933'). Required if entity_kind is set."
                },
                "importance": {
                    "type": "number",
                    "description": "Importance score 0.0–1.0 (default 0.5). Use 0.9+ for critical architecture decisions, 0.7 for useful context, 0.3 for transient notes."
                }
            },
            "required": ["content"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let content = required_str(&input, "content")?;
        let importance = input
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.5)
            .clamp(0.0, 1.0);
        let entity_kind = optional_str(&input, "entity_kind");
        let entity_name = optional_str(&input, "entity_name");

        let store = context.memory_store.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "hippocampal memory is not available — ensure the memory database is configured",
            )
        })?;

        let entity_id = if let (Some(kind), Some(name)) = (&entity_kind, &entity_name) {
            let entity = store.upsert_entity(kind, name, content).map_err(|e| {
                ToolError::execution_failed(format!("failed to upsert entity: {e}"))
            })?;
            Some(entity.id)
        } else {
            None
        };

        let session_id = Some(context.state_namespace.as_str());
        store
            .insert_fact(entity_id.as_deref(), &content, "memorize", importance, session_id)
            .map_err(|e| ToolError::execution_failed(format!("failed to store fact: {e}")))?;

        let mut detail = format!("Memorized (importance={importance:.1})");
        if let Some(ref kind) = entity_kind {
            if let Some(ref name) = entity_name {
                detail.push_str(&format!(" — linked to {kind} '{name}'"));
            }
        }
        Ok(ToolResult::success(detail))
    }
}
