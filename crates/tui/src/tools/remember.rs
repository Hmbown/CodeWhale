//! `remember` tool — model-callable capture into the native memory store.
//!
//! Lets the model itself notice a durable preference, convention, or fact
//! worth keeping across sessions and write it to the native memory store
//! (Markdown + SQLite FTS5 under `memory/global/MEMORY.md`). The tool is
//! auto-approved and side-effecting only on the user-owned memory files,
//! so it doesn't get gated behind the same approval flow as shell or
//! arbitrary file writes.
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
                },
                "scope": {
                    "type": "string",
                    "enum": ["global", "workspace"],
                    "description": "Native backend scope; defaults to global."
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

        if let Some(store) = crate::native_memory::NativeMemoryStore::from_global_path(path) {
            let scope = match input
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("global")
            {
                "global" => crate::native_memory::MemoryScope::Global,
                "workspace" => crate::native_memory::MemoryScope::Workspace,
                other => {
                    return Err(ToolError::invalid_input(format!(
                        "unknown memory scope `{other}`; expected global or workspace"
                    )));
                }
            };
            let workspace_id = if scope == crate::native_memory::MemoryScope::Workspace {
                Some(
                    crate::native_memory::NativeMemoryStore::workspace_id(&context.workspace)
                        .map_err(|error| {
                            ToolError::execution_failed(format!(
                                "failed to resolve workspace memory scope: {error}"
                            ))
                        })?
                        .ok_or_else(|| {
                            ToolError::execution_failed(
                                "workspace memory requires a git repository with an origin",
                            )
                        })?,
                )
            } else {
                None
            };
            let hit = store
                .remember(scope, workspace_id.as_deref(), note)
                .map_err(|error| {
                    ToolError::execution_failed(format!("failed to write native memory: {error}"))
                })?;
            return Ok(ToolResult::success(format!(
                "remembered in native memory: {}:{}-{}",
                hit.source.display(),
                hit.line_start,
                hit.line_end
            ))
            .with_metadata(json!({
                "memory_backend": "native",
                "scope": if scope == crate::native_memory::MemoryScope::Global { "global" } else { "workspace" },
                "source": hit.source,
                "line_start": hit.line_start,
                "line_end": hit.line_end,
                "untrusted": true
            })));
        }

        Err(ToolError::execution_failed(format!(
            "native memory store not found at {} — expected the              `memory/global/MEMORY.md` layout; the legacy single-file memory              path was removed in v0.9.4",
            path.display()
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
    async fn rejects_legacy_plain_file_memory_path() {
        // The legacy single-file path (`memory.md`) was removed in v0.9.4;
        // only the native `memory/global/MEMORY.md` layout is writable.
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("memory.md");
        let ctx = ctx_with_memory(path);

        let tool = RememberTool;
        let err = tool
            .execute(json!({"note": "use 4 spaces for indentation"}), &ctx)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("native memory store"), "{err}");
    }

    #[tokio::test]
    async fn native_backend_capture_updates_markdown_and_fts_index() {
        let tmp = tempdir().unwrap();
        let root = tmp.path().join("memory");
        let path = root.join("global/MEMORY.md");
        let mut ctx = ToolContext::new(tmp.path());
        ctx.memory_path = Some(path);

        let result = RememberTool
            .execute(
                json!({"note": "Prefer bounded receipts", "scope": "global"}),
                &ctx,
            )
            .await
            .expect("native capture should succeed");
        assert!(result.success);
        assert_eq!(result.metadata.unwrap()["memory_backend"], "native");

        let hits = crate::native_memory::NativeMemoryStore::new(root)
            .search("receipts", 5)
            .expect("native capture should update index");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].text, "Prefer bounded receipts");
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
