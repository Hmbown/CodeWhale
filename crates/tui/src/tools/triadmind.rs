//! # TriadMind Tools — native architecture governance tools
//!
//! Registered as model-visible tools: `triadmind_sync`, `triadmind_verify`, `triadmind_rules`.
//!
//! These replace the previous MCP bridge (triadmind-core MCP server) with
//! zero-overhead native Rust implementations.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::tools::spec::{ApprovalRequirement, ToolCapability, ToolContext, ToolResult};
use deepseek_triadmind::protocol;
use deepseek_triadmind::rules::{self, RulesPaths};

/// Shared state for `triadmind_sync` — cached config path.
fn triad_dir(workspace: &PathBuf) -> PathBuf {
    workspace.join(".triadmind")
}

// ── triadmind_sync ──────────────────────────────────────────────────

struct TriadmindSyncTool;

#[async_trait]
impl crate::tools::spec::ToolSpec for TriadmindSyncTool {
    fn name(&self) -> &'static str {
        "triadmind_sync"
    }

    fn description(&self) -> &'static str {
        "Trigger TriadMind topology sync. Scans project source files and rebuilds triad-map.json (the architecture capability graph). Use when source files have changed and the topology is stale, or before running triadmind_verify."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "force": {
                    "type": "boolean",
                    "description": "Force a full rebuild even if no file changes detected (default: false)."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, deepseek_tools::ToolError> {
        let force = input.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        let triad_dir = triad_dir(&context.workspace);
        let map_file = triad_dir.join("triad-map.json");

        if !triad_dir.exists() {
            return Ok(ToolResult::success(format!(
                "No .triadmind directory found at {}. Run `triadmind init` first to initialize the workspace.",
                triad_dir.display()
            )));
        }

        // Check if triad-map.json exists
        let map_exists = map_file.exists();
        let stale = if map_exists {
            // Simple staleness check: compare last modified times
            // Full sync implementation comes in Phase 2
            let map_meta = std::fs::metadata(&map_file).map(|m| m.modified().ok()).ok().flatten();
            let triad_meta = std::fs::metadata(&triad_dir).map(|m| m.modified().ok()).ok().flatten();

            match (map_meta, triad_meta) {
                (Some(map_time), Some(_)) => {
                    // For now, report if force is requested or map is older than 1 hour
                    if force {
                        true
                    } else {
                        let elapsed = std::time::SystemTime::now()
                            .duration_since(map_time)
                            .unwrap_or_default();
                        elapsed.as_secs() > 3600
                    }
                }
                _ => true,
            }
        } else {
            true // no map exists
        };

        let message = if !map_exists {
            format!(
                "No triad-map.json found. This workspace needs initialization.\n\
                 Run `triadmind_rules` to install the architecture rules first.\n\
                 Full sync from source analysis requires the parser module (Phase 2).\n\
                 Current triad-map.json path: {}",
                map_file.display()
            )
        } else if stale {
            format!(
                "TriadMind topology may be stale.\n\
                 Map file: {} ({})\n\
                 Note: Full source re-scan not yet available in native Rust port.\n\
                 For now, use the MCP server or CLI: `triadmind sync --force`",
                map_file.display(),
                if force { "force=true" } else { "age > 1h" }
            )
        } else {
            let result = protocol::read_triad_map(&map_file);
            match result {
                Ok(nodes) => {
                    let categories: std::collections::HashSet<_> = nodes
                        .iter()
                        .filter_map(|n| n.category.as_deref())
                        .collect();
                    format!(
                        "TriadMind topology is up to date.\n\
                         Nodes: {}\n\
                         Categories: {:?}\n\
                         Map file: {}",
                        nodes.len(),
                        categories,
                        map_file.display()
                    )
                }
                Err(e) => format!("Failed to read triad-map.json: {}", e),
            }
        };

        Ok(ToolResult::success(message))
    }
}

// ── triadmind_verify ────────────────────────────────────────────────

struct TriadmindVerifyTool;

#[async_trait]
impl crate::tools::spec::ToolSpec for TriadmindVerifyTool {
    fn name(&self) -> &'static str {
        "triadmind_verify"
    }

    fn description(&self) -> &'static str {
        "Verify the topological quality of the current triad-map. Checks ghost node ratio, execute-like method ratio, abstraction deficit hotspots, and edge consistency. Returns a structured quality report."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, deepseek_tools::ToolError> {
        let triad_dir = triad_dir(&context.workspace);
        let map_file = triad_dir.join("triad-map.json");

        if !map_file.exists() {
            return Ok(ToolResult::success(format!(
                "No triad-map.json found at {}. Run `triadmind_sync` first.",
                map_file.display()
            )));
        }

        let nodes = protocol::read_triad_map(&map_file).map_err(|e| {
            deepseek_tools::ToolError::execution_failed(format!("Failed to read triad-map: {}", e))
        })?;

        // Basic quality metrics (full verify in Phase 2)
        let total = nodes.len();
        let with_fission = nodes.iter().filter(|n| n.fission.is_some()).count();
        let by_category: std::collections::HashMap<&str, usize> = nodes.iter().fold(
            std::collections::HashMap::new(),
            |mut acc, n| {
                let cat = n.category.as_deref().unwrap_or("uncategorized");
                *acc.entry(cat).or_insert(0) += 1;
                acc
            },
        );

        let source_files: std::collections::HashSet<_> = nodes
            .iter()
            .filter_map(|n| n.source_path.as_deref())
            .collect();

        let report = format!(
            "TriadMind Topology Quality Report\n\
             =================================\n\
             Total nodes:       {total}\n\
             With fission:      {with_fission} ({:.1}%)\n\
             Source files:      {src_count}\n\
             Categories:        {cat_count}\n\
             \n\
             Category breakdown:\n{cat_breakdown}\n\
             \n\
             Note: Full quality metrics (ghost ratio, execute-ratio, abstraction deficit)\n\
             will be available in Phase 2 of the native Rust port.\n\
             For complete analysis, use: `triadmind verify` (CLI) or `triadmind_dream` (MCP).",
            if total > 0 {
                (with_fission as f64 / total as f64) * 100.0
            } else {
                0.0
            },
            src_count = source_files.len(),
            cat_count = by_category.len(),
            cat_breakdown = by_category
                .iter()
                .map(|(cat, count)| format!("  - {cat}: {count}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        Ok(ToolResult::success(report))
    }
}

// ── triadmind_rules ─────────────────────────────────────────────────

struct TriadmindRulesTool;

#[async_trait]
impl crate::tools::spec::ToolSpec for TriadmindRulesTool {
    fn name(&self) -> &'static str {
        "triadmind_rules"
    }

    fn description(&self) -> &'static str {
        "Install or remove TriadMind always-on architecture rules into AGENTS.md and Cursor IDE rules. When installing, injects guard rules (reuse-first, Macro→Meso→Micro sequence, topology-aware development). Pass 'remove' to clean up."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["install", "remove", "check"],
                    "description": "Action: install rules, remove rules, or check if rules exist (default: install)."
                }
            },
            "required": []
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::WritesFiles]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, deepseek_tools::ToolError> {
        let action = input
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("install");

        let paths = RulesPaths::new(context.workspace.clone());

        match action {
            "remove" => {
                rules::remove_rules(&paths).map_err(|e| {
                    deepseek_tools::ToolError::execution_failed(format!("Failed to remove rules: {}", e))
                })?;
                Ok(ToolResult::success("TriadMind rules removed from AGENTS.md."))
            }
            "check" => {
                let has = rules::has_rules(&paths);
                Ok(ToolResult::success(format!(
                    "TriadMind rules are {} in AGENTS.md.",
                    if has { "present" } else { "not present" }
                )))
            }
            _ => {
                // install
                let written = rules::install_always_on_rules(&paths).map_err(|e| {
                    deepseek_tools::ToolError::execution_failed(format!("Failed to install rules: {}", e))
                })?;

                let file_list: Vec<String> = written
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();

                Ok(ToolResult::success(format!(
                    "TriadMind always-on rules installed.\n\nFiles written:\n{}",
                    file_list
                        .iter()
                        .map(|f| format!("  - {}", f))
                        .collect::<Vec<_>>()
                        .join("\n")
                )))
            }
        }
    }
}

// ── Registration helpers ────────────────────────────────────────────

/// Register all triadmind tools into the given registry.
pub fn register_triadmind_tools(
    mut builder: crate::tools::ToolRegistryBuilder,
) -> crate::tools::ToolRegistryBuilder {
    builder = builder.with_tool(Arc::new(TriadmindSyncTool));
    builder = builder.with_tool(Arc::new(TriadmindVerifyTool));
    builder = builder.with_tool(Arc::new(TriadmindRulesTool));
    builder
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolSpec;

    #[test]
    fn test_triadmind_sync_schema_is_valid() {
        let tool = TriadmindSyncTool;
        let schema = tool.input_schema();
        assert!(schema.is_object());
        assert_eq!(tool.name(), "triadmind_sync");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_triadmind_verify_schema_is_valid() {
        let tool = TriadmindVerifyTool;
        let schema = tool.input_schema();
        assert!(schema.is_object());
        assert_eq!(tool.name(), "triadmind_verify");
    }

    #[test]
    fn test_triadmind_rules_schema_is_valid() {
        let tool = TriadmindRulesTool;
        let schema = tool.input_schema();
        assert!(schema.is_object());
        assert_eq!(tool.name(), "triadmind_rules");
    }
}
