//! `remember` tool — model-callable bullet-add into the user memory file.
//!
//! Lets the model itself notice a durable preference, convention, or fact
//! worth keeping across sessions and write it to the user's `memory.md`.
//! The tool is auto-approved and side-effecting only on the user-owned
//! memory file (`~/.deepseek/memory.md` by default), so it doesn't get
//! gated behind the same approval flow as shell or arbitrary file writes.
//!
//! Only registered when `[memory] enabled = true` (or
//! `DEEPSEEK_MEMORY=on`). When disabled, the tool isn't surfaced to the
//! model at all, so prompts that mention `remember` simply fall through.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec, required_str,
};

/// Tool that appends one bullet to the user memory file.
pub struct RememberTool;

#[async_trait]
impl ToolSpec for RememberTool {
    fn name(&self) -> &'static str {
        "remember"
    }

    fn description(&self) -> &'static str {
        "Append a durable note to the user memory file so it surfaces in \
         future sessions. Use this when the user states a preference, a \
         convention they want enforced, or a fact about themselves or \
         their workflow that you should not have to relearn next time. \
         Keep notes terse (one sentence). Don't store secrets, transient \
         tasks, or reasoning scratch — those belong in a checklist or in \
         the conversation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "note": {
                    "type": "string",
                    "description": "The single-sentence durable note to remember."
                }
            },
            "required": ["note"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        // Memory writes are scoped to the user's own memory file; gating
        // them behind the standard shell/write approval would defeat the
        // point of automatic memory.
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let note = required_str(&input, "note")?;
        let path = context.memory_path.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "user memory is disabled — set `[memory] enabled = true` in config.toml or \
                 `DEEPSEEK_MEMORY=on` in the environment to enable",
            )
        })?;

        // Write to flat memory.md (primary storage, backward compatible).
        crate::memory::append_entry(path, note).map_err(|err| {
            ToolError::execution_failed(format!("failed to append to {}: {err}", path.display()))
        })?;

        // Write to vector DB for semantic retrieval (best-effort).
        if let Some(ref vdb) = context.vector_db {
            // #5: memory dedup — check for existing similar memories
            // before writing. If an existing memory matches the new note
            // with score > 0.85, skip the write to avoid polluting the
            // vector store with near-duplicates.
            let existing = vdb
                .search_memories(note, 3, None)
                .await
                .unwrap_or_default();
            // #5: dedup by vector similarity score alone — cosine >= 0.85
            // signals near-duplicate semantic content regardless of exact
            // wording. The `contains` substring check was removed because it
            // misses inversions (old content broader than new), a common case
            // when the model progressively refines its understanding.
            let too_similar = existing.iter().any(|m| m.score > 0.85);

            if too_similar {
                tracing::debug!(
                    note = %note,
                    "remember tool: skipping duplicate memory (score > 0.85)"
                );
            } else {
                let item = crate::vector_db::NewMemoryItem {
                    content: note.to_string(),
                    source: "model".to_string(),
                    session_id: context.state_namespace.clone(),
                    tags: Some("remembered".to_string()),
                    ttl: None,
                };
                if let Err(e) = vdb.store_memory(item).await {
                    tracing::warn!(
                        "remember tool: failed to store memory in vector db: {e}"
                    );
                }
            }
        }

        Ok(ToolResult::success(format!(
            "remembered: {}",
            note.trim_start_matches('#').trim()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ctx_with_memory(path: PathBuf) -> ToolContext {
        let mut ctx = ToolContext::new(path.parent().unwrap_or_else(|| std::path::Path::new(".")));
        ctx.memory_path = Some(path);
        ctx
    }

    #[tokio::test]
    async fn returns_error_when_memory_disabled() {
        let tmp = tempdir().unwrap();
        let mut ctx = ToolContext::new(tmp.path());
        ctx.memory_path = None; // explicitly disabled

        let tool = RememberTool;
        let err = tool
            .execute(json!({"note": "use 4 spaces for indentation"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("memory is disabled"), "{err}");
    }

    #[tokio::test]
    async fn appends_bullet_to_memory_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let ctx = ctx_with_memory(path.clone());

        let tool = RememberTool;
        let result = tool
            .execute(json!({"note": "use 4 spaces for indentation"}), &ctx)
            .await
            .expect("ok");
        assert!(result.success);
        assert!(result.content.contains("4 spaces"));

        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("4 spaces"));
        assert!(body.starts_with("- ("), "{body}");
    }

    #[tokio::test]
    async fn rejects_missing_note_field() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let ctx = ctx_with_memory(path);

        let tool = RememberTool;
        let err = tool.execute(json!({}), &ctx).await.unwrap_err();
        assert!(err.to_string().to_lowercase().contains("note"), "{err}");
    }
}
