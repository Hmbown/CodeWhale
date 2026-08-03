//! Canonical action-based wrapper for file system tools.
//!
//! The model sees one tool: `File` with an `action` parameter
//! (read | list | search_name | search_content | write | edit | patch).
//! The per-action legacy execution aliases were removed in v0.9.3.

use async_trait::async_trait;
use serde_json::{Value, json};

use super::apply_patch::ApplyPatchTool;
use super::file::{EditFileTool, ListDirTool, ReadFileTool, WriteFileTool};
use super::file_search::FileSearchTool;
use super::search::GrepFilesTool;
use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

pub struct FileTool {
    name: &'static str,
    forced_action: Option<&'static str>,
    allow_writes: bool,
    allow_patch: bool,
}

impl FileTool {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            allow_writes: true,
            allow_patch: false,
        }
    }

    pub const fn with_patch(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            allow_writes: true,
            allow_patch: true,
        }
    }

    pub const fn read_only(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            allow_writes: false,
            allow_patch: false,
        }
    }

    pub const fn alias(name: &'static str, action: &'static str) -> Self {
        Self {
            name,
            forced_action: Some(action),
            allow_writes: true,
            allow_patch: true,
        }
    }

    fn resolve_action<'a>(&self, input: &'a Value) -> &'a str {
        self.forced_action.unwrap_or_else(|| {
            input
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("read")
        })
    }

    fn strip_action(&self, input: Value) -> Result<Value, ToolError> {
        let mut input = input;
        if let Some(obj) = input.as_object_mut() {
            obj.remove("action");
            Ok(input)
        } else {
            Err(ToolError::invalid_input(
                "File tool input must be an object",
            ))
        }
    }

    fn validate_edit_shape(&self, input: &Value) -> Result<(), ToolError> {
        let Some(replace) = input.get("replace") else {
            return Ok(());
        };
        if replace.is_string() {
            return Ok(());
        }

        let json_type = match replace {
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        let mut preview = replace.to_string();
        if preview.chars().count() > 120 {
            preview = preview.chars().take(117).collect::<String>() + "...";
        }

        Err(ToolError::invalid_input(format!(
            "Invalid File action=edit input: `replace` must be a string (shape: {{\"action\":\"edit\",\"path\":\"...\",\"search\":\"...\",\"replace\":\"...\"}}), got {json_type}. Received: {preview}. For full-file replacements, use action=patch with `files` (or deprecated alias `changes`)."
        )))
    }
}

#[async_trait]
impl ToolSpec for FileTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        self.name == "File"
    }

    fn description(&self) -> &'static str {
        "Read, list, search, write, edit, or patch workspace files. Use read before edit; edit performs one exact replacement, while patch is best for multi-hunk or multi-file changes. Read/list/search actions are parallel-safe and do not require approval. Available actions depend on the active mode and feature policy."
    }

    fn input_schema(&self) -> Value {
        let mut actions = vec!["read", "list", "search_name", "search_content"];
        if self.allow_writes {
            actions.extend(["write", "edit"]);
        }
        if self.allow_patch {
            actions.push("patch");
        }
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": actions,
                    "description": "Action to perform"
                },
                "path": {
                    "type": "string",
                    "description": "File or directory path (read, list, search, write, edit, patch)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Starting line for read (1-based, default 1)"
                },
                "max_lines": {
                    "type": "integer",
                    "description": "Maximum lines to return for read or blame (default 200)"
                },
                "pages": {
                    "type": "string",
                    "description": "PDF page range for read, e.g. '1-5'"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write for action=write"
                },
                "search": {
                    "type": "string",
                    "description": "Exact text to search for action=edit"
                },
                "replace": {
                    "type": "string",
                    "description": "Replacement text for action=edit"
                },
                "files": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] },
                    "description": "Full-file replacement list for action=patch"
                },
                "fuzz": {
                    "oneOf": [{ "type": "boolean" }, { "type": "integer" }],
                    "description": "Fuzzy matching flag for edit or max fuzz for patch"
                },
                "query": {
                    "type": "string",
                    "description": "Search query for search_name or search_content"
                },
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern for search_content"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum results for search_name"
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum results for search_content or search_name"
                },
                "extensions": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional extension filter for search_name"
                },
                "include": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns to include for search_content"
                },
                "exclude": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Glob patterns to exclude for search_name or search_content"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Context lines around each match for search_content"
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive matching for search_content"
                },
                "patch": {
                    "type": "string",
                    "description": "Unified diff patch content for action=patch"
                },
                "changes": {
                    "type": "array",
                    "items": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } }, "required": ["path", "content"] },
                    "description": "Deprecated alias for files in action=patch"
                },
                "create_if_missing": {
                    "type": "boolean",
                    "description": "Create files if missing for action=patch"
                }
            },
            "required": ["action"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        let mut capabilities = vec![ToolCapability::ReadOnly, ToolCapability::Sandboxable];
        let can_mutate = match self.forced_action {
            Some("write" | "edit" | "patch") => true,
            Some(_) => false,
            None => self.allow_writes || self.allow_patch,
        };
        if can_mutate {
            capabilities.extend([
                ToolCapability::WritesFiles,
                ToolCapability::RequiresApproval,
            ]);
        }
        capabilities
    }

    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match self.resolve_action(input) {
            "read" | "list" | "search_name" | "search_content" => ApprovalRequirement::Auto,
            "write" | "edit" | "patch" => ApprovalRequirement::Suggest,
            _ => ApprovalRequirement::Auto,
        }
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(
            self.resolve_action(input),
            "read" | "list" | "search_name" | "search_content"
        )
    }

    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(
            self.resolve_action(input),
            "read" | "list" | "search_name" | "search_content"
        )
    }

    fn starts_detached_for(&self, _input: &Value) -> bool {
        false
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let action = self.resolve_action(&input).to_string();
        if matches!(action.as_str(), "write" | "edit") && !self.allow_writes {
            return Err(ToolError::not_available(
                "File writes are unavailable in the current mode",
            ));
        }
        if action == "patch" && !self.allow_patch {
            return Err(ToolError::not_available(
                "File.patch is unavailable because the patch feature is disabled",
            ));
        }
        if action == "edit" {
            self.validate_edit_shape(&input)?;
        }
        let mut input = self.strip_action(input)?;

        match action.as_str() {
            "read" => ReadFileTool.execute(input, context).await,
            "list" => ListDirTool.execute(input, context).await,
            "search_name" => {
                if let Some(obj) = input.as_object_mut()
                    && !obj.contains_key("limit")
                    && let Some(max) = obj.get("max_results").cloned()
                {
                    obj.insert("limit".to_string(), max);
                }
                FileSearchTool.execute(input, context).await
            }
            "search_content" => {
                if let Some(obj) = input.as_object_mut() {
                    if !obj.contains_key("pattern")
                        && let Some(query) = obj.get("query").cloned()
                    {
                        obj.insert("pattern".to_string(), query);
                    }
                    if !obj.contains_key("max_results")
                        && let Some(limit) = obj.get("limit").cloned()
                    {
                        obj.insert("max_results".to_string(), limit);
                    }
                }
                GrepFilesTool.execute(input, context).await
            }
            "write" => WriteFileTool.execute(input, context).await,
            "edit" => EditFileTool.execute(input, context).await,
            "patch" => ApplyPatchTool.execute(input, context).await,
            other => Err(ToolError::invalid_input(format!(
                "Unknown File action: {other}"
            ))),
        }
    }
}
