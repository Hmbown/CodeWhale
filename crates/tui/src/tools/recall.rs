//! `recall` tool — query the hippocampal memory store.
//!
//! Performs full-text search over stored facts, optionally scoped to a
//! namespace, and returns related entities, relations, and glossary tags.
//! This is the retrieval side of the hippocampal memory system.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_u64,
};

/// Tool that queries the hippocampal memory store.
pub struct RecallTool;

#[async_trait]
impl ToolSpec for RecallTool {
    fn name(&self) -> &'static str {
        "recall"
    }

    fn description(&self) -> &'static str {
        "Search long-term memory for facts and entities learned in previous sessions. \
         Use this when you need to remember project context, user preferences, \
         architecture decisions, or anything stored with `memorize`. \
         Results include facts, related entities, relationships, and glossary tags. \
         Optionally scope the search to a namespace for workspace isolation. \
         The more specific your query, the better the results."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Search query — use key terms like 'indentation', 'database schema', 'deployment config'"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of results (default 5, max 20)"
                },
                "include_graph": {
                    "type": "boolean",
                    "description": "Also return related entities and relationships (default true)"
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional namespace to scope the search (e.g. 'workspace:/path/to/project'). Only facts within this namespace are returned."
                },
                "glossary_tag": {
                    "type": "string",
                    "description": "Optional glossary tag to filter facts by (e.g. 'rate-limit'). Only facts tagged with this term are returned."
                }
            },
            "required": ["query"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let query = input
            .get("query")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ToolError::missing_field("query"))?;

        let limit = optional_u64(&input, "limit", 5).min(20) as usize;
        let include_graph = input
            .get("include_graph")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let namespace = input
            .get("namespace")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());
        let glossary_tag = input
            .get("glossary_tag")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let store = context.memory_store.as_ref().ok_or_else(|| {
            ToolError::execution_failed("hippocampal memory is not available")
        })?;

        // Resolve namespace to ID if provided
        let namespace_id = if let Some(ns_name) = namespace {
            // Search by name to find existing namespace
            let nss = store.list_namespaces().map_err(|e| {
                ToolError::execution_failed(format!("failed to list namespaces: {e}"))
            })?;
            nss.into_iter().find(|ns| ns.name == ns_name)
        } else {
            None
        };

        // Search facts — scoped by namespace if provided
        let facts = if let Some(ref ns) = namespace_id {
            store.search_facts_in_namespace(query, &ns.id, limit)
        } else {
            store.search_facts(query, limit)
        }
        .map_err(|e| ToolError::execution_failed(format!("memory search failed: {e}")))?;

        // If glossary_tag filter is provided, further filter
        let facts = if let Some(tag) = glossary_tag {
            // Find the glossary term
            let terms = store.search_glossary(tag, 5).map_err(|e| {
                ToolError::execution_failed(format!("glossary search failed: {e}"))
            })?;
            if let Some(term) = terms.into_iter().find(|t| t.term == tag) {
                let tagged = store.search_facts_by_glossary(&term.id, limit).map_err(|e| {
                    ToolError::execution_failed(format!("tagged search failed: {e}"))
                })?;
                // Intersection with current facts
                let tagged_ids: std::collections::HashSet<String> =
                    tagged.into_iter().map(|f| f.id).collect();
                facts.into_iter().filter(|f| tagged_ids.contains(&f.id)).collect()
            } else {
                Vec::new()
            }
        } else {
            facts
        };

        // Search entities
        let entities = if let Some(ref ns) = namespace_id {
            store.search_entities_in_namespace(query, &ns.id, limit)
        } else {
            store.search_entities(query, limit)
        }
        .map_err(|e| ToolError::execution_failed(format!("entity search failed: {e}")))?;

        if facts.is_empty() && entities.is_empty() {
            // Fallback: return top important facts as a hint
            let top = if let Some(ref ns) = namespace_id {
                store.important_facts_in_namespace(&ns.id, 3)
            } else {
                store.important_facts(3)
            }
            .map_err(|_| ());

            if let Ok(top_facts) = top && !top_facts.is_empty() {
                let mut result = format!("No results for '{query}'.\n\nTop stored facts:\n");
                for (i, f) in top_facts.iter().enumerate() {
                    result.push_str(&format!("{}. [imp={:.1}] {}\n", i + 1, f.importance, f.content));
                }
                return Ok(ToolResult::success(result));
            }
            return Ok(ToolResult::success(format!(
                "No memory results for '{query}'. Use `memorize` to store facts."
            )));
        }

        let mut output = String::new();

        // Facts with glossary tags
        if !facts.is_empty() {
            output.push_str(&format!("Facts ({}):\n", facts.len()));
            for (i, f) in facts.iter().enumerate() {
                output.push_str(&format!("{}. [imp={:.1}] {}\n", i + 1, f.importance, f.content));

                // Show linked entity
                if let Some(ref eid) = f.entity_id {
                    if let Ok(Some(e)) = store.get_entity(eid) {
                        output.push_str(&format!("   → linked to {} '{}'\n", e.kind, e.name));
                    }
                }

                // Show glossary tags
                if let Ok(tags) = store.get_fact_glossary_terms(&f.id) {
                    if !tags.is_empty() {
                        let tag_names: Vec<&str> = tags.iter().map(|t| t.term.as_str()).collect();
                        output.push_str(&format!("   → tags: [{}]\n", tag_names.join(", ")));
                    }
                }
            }
        }

        // Entities
        if !entities.is_empty() {
            if !output.is_empty() {
                output.push('\n');
            }
            output.push_str(&format!("Entities ({}):\n", entities.len()));
            for (i, e) in entities.iter().enumerate() {
                output.push_str(&format!("{}. [{}] {} — {}\n", i + 1, e.kind, e.name, e.description));
            }
        }

        // Graph walk
        if include_graph {
            for e in &entities {
                if let Ok(rels) = store.relations_for_entity(&e.id, 5)
                    && !rels.is_empty()
                {
                    output.push_str(&format!("\nRelations for '{}':\n", e.name));
                    for r in &rels {
                        let target_name = store
                            .get_entity(&r.target_id)
                            .ok()
                            .flatten()
                            .map(|e| e.name)
                            .unwrap_or_default();
                        let source_name = store
                            .get_entity(&r.source_id)
                            .ok()
                            .flatten()
                            .map(|e| e.name)
                            .unwrap_or_default();
                        output.push_str(&format!(
                            "  {} ──{}({:.1})──▶ {}\n",
                            source_name, r.kind, r.strength, target_name
                        ));
                    }
                }
            }
        }

        Ok(ToolResult::success(output))
    }
}
