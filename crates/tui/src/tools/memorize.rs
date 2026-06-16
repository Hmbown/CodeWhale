//! `memorize` tool — store a structured fact in the hippocampal memory store.
//!
//! Unlike the simpler `remember` tool (which appends a bullet to `memory.md`),
//! `memorize` records a fact in the SQLite-backed entity graph with importance
//! scoring, optional entity binding, glossary tags, and namespace isolation.
//! Facts stored here survive compaction and can be recalled across sessions.

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
         (file, issue, person) for graph-based recall, add glossary tags for \
         cross-referencing, or scope to a namespace for workspace isolation. \
         High-importance facts (0.8+) are retained indefinitely; low-importance \
         facts may be pruned over time."
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
                },
                "glossary_tags": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of keyword tags for cross-referencing (e.g. ['rate-limit', 'api', 'config']). Tags are auto-created if they don't exist."
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional namespace for workspace isolation (e.g. 'workspace:/path/to/project'). Facts in different namespaces don't interfere."
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
        let namespace = optional_str(&input, "namespace");
        let glossary_tags: Vec<String> = input
            .get("glossary_tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        let store = context.memory_store.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "hippocampal memory is not available — ensure the memory database is configured",
            )
        })?;

        // Resolve namespace
        let namespace_id = if let Some(ref ns_name) = namespace {
            Some(
                store
                    .get_or_create_workspace_namespace(ns_name)
                    .map_err(|e| {
                        ToolError::execution_failed(format!("failed to resolve namespace: {e}"))
                    })?,
            )
        } else {
            None
        };

        // Upsert entity if provided
        let entity_id = if let (Some(kind), Some(name)) = (&entity_kind, &entity_name) {
            if let Some(ref ns) = namespace_id {
                let entity = store
                    .upsert_entity_in_namespace(kind, name, content, Some(&ns.id))
                    .map_err(|e| {
                        ToolError::execution_failed(format!("failed to upsert entity: {e}"))
                    })?;
                Some(entity.id)
            } else {
                let entity = store.upsert_entity(kind, name, content).map_err(|e| {
                    ToolError::execution_failed(format!("failed to upsert entity: {e}"))
                })?;
                Some(entity.id)
            }
        } else {
            None
        };

        // Insert the fact (with namespace if applicable)
        let session_id = Some(context.state_namespace.as_str());
        let fact = if let Some(ref ns) = namespace_id {
            store
                .insert_fact_in_namespace(
                    entity_id.as_deref(),
                    &content,
                    "memorize",
                    importance,
                    session_id,
                    Some(&ns.id),
                )
                .map_err(|e| ToolError::execution_failed(format!("failed to store fact: {e}")))?
        } else {
            store
                .insert_fact(entity_id.as_deref(), &content, "memorize", importance, session_id)
                .map_err(|e| ToolError::execution_failed(format!("failed to store fact: {e}")))?
        };

        // Link glossary tags
        let mut linked_tags = Vec::new();
        for tag in &glossary_tags {
            if let Ok(term) = store.add_glossary_term(
                tag,
                "",
                "general",
                namespace_id.as_ref().map(|ns| ns.id.as_str()),
            ) {
                let _ = store.link_fact_glossary(&fact.id, &term.id);
                linked_tags.push(tag.clone());
            }
        }

        // Build response
        let mut detail = format!("Memorized (importance={importance:.1})");
        if let Some(ref kind) = entity_kind {
            if let Some(ref name) = entity_name {
                detail.push_str(&format!(" — linked to {kind} '{name}'"));
            }
        }
        if !linked_tags.is_empty() {
            detail.push_str(&format!(" — tags: [{}]", linked_tags.join(", ")));
        }
        if namespace.is_some() {
            detail.push_str(" — namespaced");
        }
        Ok(ToolResult::success(detail))
    }
}
