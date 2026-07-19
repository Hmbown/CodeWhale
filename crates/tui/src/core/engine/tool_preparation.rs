//! Side-effect-free preparation of concrete tool inputs.
//!
//! The turn loop remains the authority orchestrator. This module only makes
//! the input-specific policy decision inspectable and reusable, including a
//! mandatory second preparation after a hook rewrites input.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;

use crate::mcp::McpPool;
use crate::tools::ToolRegistry;
use crate::tools::spec::{ApprovalRequirement, PreparedToolCall, ResourceClaim, ToolError};

use super::dispatch::{
    mcp_tool_approval_description, mcp_tool_is_parallel_safe, mcp_tool_is_read_only,
};
use super::tool_catalog::{CODE_EXECUTION_TOOL_NAME, JS_EXECUTION_TOOL_NAME, is_tool_search_tool};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct PreparedToolPolicy {
    pub(super) call: PreparedToolCall,
    pub(super) auto_approve: bool,
}

/// Prepare a concrete call without mutating external state.
pub(super) fn prepare_tool_call(
    name: &str,
    input: Value,
    registry: Option<&ToolRegistry>,
    session_auto_approve: bool,
) -> Result<PreparedToolPolicy, ToolError> {
    if McpPool::is_mcp_tool(name) {
        let read_only = mcp_tool_is_read_only(name);
        return Ok(PreparedToolPolicy {
            call: PreparedToolCall {
                name: name.to_string(),
                input,
                description: mcp_tool_approval_description(name),
                read_only,
                supports_parallel: mcp_tool_is_parallel_safe(name),
                starts_detached: false,
                approval: if read_only {
                    ApprovalRequirement::Auto
                } else {
                    ApprovalRequirement::Suggest
                },
                resources: vec![ResourceClaim::GlobalExclusive],
            },
            auto_approve: session_auto_approve,
        });
    }

    if let Some(registry) = registry
        && let Some(spec) = registry.get(name)
    {
        let mut call = spec.prepare(input, registry.context())?;
        call.resources = registered_resource_claims(name, &call.input, registry.context())?;
        return Ok(PreparedToolPolicy {
            call,
            auto_approve: registry.context().auto_approve,
        });
    }

    if name == CODE_EXECUTION_TOOL_NAME {
        return Ok(conservative_execution_policy(
            name,
            input,
            "Run model-provided Python code in local execution sandbox",
            session_auto_approve,
        ));
    }

    if name == JS_EXECUTION_TOOL_NAME {
        return Ok(conservative_execution_policy(
            name,
            input,
            "Run model-provided JavaScript code in local Node.js execution sandbox",
            session_auto_approve,
        ));
    }

    if is_tool_search_tool(name) {
        return Ok(PreparedToolPolicy {
            call: PreparedToolCall {
                name: name.to_string(),
                input,
                description: "Search tool catalog".to_string(),
                read_only: true,
                supports_parallel: false,
                starts_detached: false,
                approval: ApprovalRequirement::Auto,
                resources: Vec::new(),
            },
            auto_approve: session_auto_approve,
        });
    }

    Err(ToolError::not_available(format!(
        "tool '{name}' has no preparation path"
    )))
}

/// Re-run preparation from the rewritten input rather than patching any
/// previously derived field.
pub(super) fn reprepare_tool_call_after_hook(
    name: &str,
    updated_input: Value,
    registry: Option<&ToolRegistry>,
    session_auto_approve: bool,
) -> Result<PreparedToolPolicy, ToolError> {
    prepare_tool_call(name, updated_input, registry, session_auto_approve)
}

fn conservative_execution_policy(
    name: &str,
    input: Value,
    description: &str,
    auto_approve: bool,
) -> PreparedToolPolicy {
    PreparedToolPolicy {
        call: PreparedToolCall {
            name: name.to_string(),
            input,
            description: description.to_string(),
            read_only: false,
            supports_parallel: false,
            starts_detached: false,
            approval: ApprovalRequirement::Suggest,
            resources: vec![ResourceClaim::GlobalExclusive],
        },
        auto_approve,
    }
}

fn registered_resource_claims(
    name: &str,
    input: &Value,
    context: &crate::tools::ToolContext,
) -> Result<Vec<ResourceClaim>, ToolError> {
    match name {
        "read_file" => path_claim(input, "path", None, context, ResourceClaim::ReadPath),
        "write_file" | "edit_file" => {
            path_claim(input, "path", None, context, ResourceClaim::WritePath)
        }
        "list_dir" | "grep_files" | "file_search" => {
            path_claim(input, "path", Some("."), context, ResourceClaim::ReadTree)
        }
        "apply_patch" => apply_patch_resource_claims(input, context),
        "terminal/run" => Ok(terminal_claim(input, "session", Some("term-1"))),
        "terminal/send" | "terminal/wait" | "terminal/cancel" | "terminal/reset" => {
            Ok(terminal_claim(input, "session", None))
        }
        "exec_shell_wait"
        | "exec_wait"
        | "exec_shell_interact"
        | "exec_interact"
        | "exec_shell_cancel" => Ok(terminal_claim(input, "task_id", None)),
        _ => Ok(global_exclusive_claim()),
    }
}

fn path_claim(
    input: &Value,
    key: &str,
    default: Option<&str>,
    context: &crate::tools::ToolContext,
    build: fn(PathBuf) -> ResourceClaim,
) -> Result<Vec<ResourceClaim>, ToolError> {
    let raw = input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .or(default);
    let Some(raw) = raw else {
        return Ok(global_exclusive_claim());
    };
    Ok(context
        .resolve_path(raw)
        .map_or_else(|_| global_exclusive_claim(), |path| vec![build(path)]))
}

fn apply_patch_resource_claims(
    input: &Value,
    context: &crate::tools::ToolContext,
) -> Result<Vec<ResourceClaim>, ToolError> {
    let Ok(preflight) = crate::tools::apply_patch::preflight_apply_patch(input) else {
        return Ok(global_exclusive_claim());
    };
    if preflight.touched_files.is_empty() {
        return Ok(global_exclusive_claim());
    }

    let mut claims = BTreeSet::new();
    for path in preflight.touched_files {
        let Ok(path) = context.resolve_path(&path) else {
            return Ok(global_exclusive_claim());
        };
        claims.insert(ResourceClaim::WritePath(path));
    }
    Ok(claims.into_iter().collect())
}

fn terminal_claim(input: &Value, key: &str, default: Option<&str>) -> Vec<ResourceClaim> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .or(default)
        .map_or_else(global_exclusive_claim, |id| {
            vec![ResourceClaim::Terminal(id.to_string())]
        })
}

fn global_exclusive_claim() -> Vec<ResourceClaim> {
    vec![ResourceClaim::GlobalExclusive]
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::tempdir;

    use crate::tools::spec::{ToolCapability, ToolContext, ToolResult, ToolSpec};

    use super::*;

    struct InputDependentTool;

    #[async_trait]
    impl ToolSpec for InputDependentTool {
        fn name(&self) -> &str {
            "input_dependent"
        }

        fn description(&self) -> &str {
            "characterization tool"
        }

        fn input_schema(&self) -> Value {
            json!({"type": "object"})
        }

        fn capabilities(&self) -> Vec<ToolCapability> {
            vec![ToolCapability::WritesFiles]
        }

        fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
            if input.get("safe").and_then(Value::as_bool) == Some(true) {
                ApprovalRequirement::Auto
            } else {
                ApprovalRequirement::Required
            }
        }

        fn is_read_only_for(&self, input: &Value) -> bool {
            input.get("safe").and_then(Value::as_bool) == Some(true)
        }

        fn supports_parallel_for(&self, input: &Value) -> bool {
            self.is_read_only_for(input)
        }

        fn starts_detached_for(&self, input: &Value) -> bool {
            input.get("detached").and_then(Value::as_bool) == Some(true)
        }

        async fn execute(
            &self,
            _input: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult, ToolError> {
            unreachable!("preparation must not execute the tool")
        }
    }

    fn registry() -> (tempfile::TempDir, ToolRegistry) {
        let root = tempdir().expect("tempdir");
        let mut context = ToolContext::new(root.path().to_path_buf());
        context.auto_approve = true;
        let mut registry = ToolRegistry::new(context);
        registry.register(Arc::new(InputDependentTool));
        (root, registry)
    }

    #[test]
    fn prepared_policy_matches_existing_input_specific_decisions() {
        let (_root, registry) = registry();
        let spec = registry.get("input_dependent").expect("registered tool");

        for input in [
            json!({"safe": true, "detached": false}),
            json!({"safe": false, "detached": true}),
        ] {
            let prepared =
                prepare_tool_call("input_dependent", input.clone(), Some(&registry), false)
                    .expect("prepare");

            assert_eq!(
                prepared.call.approval,
                spec.approval_requirement_for(&input)
            );
            assert_eq!(prepared.call.read_only, spec.is_read_only_for(&input));
            assert_eq!(
                prepared.call.supports_parallel,
                spec.supports_parallel_for(&input)
            );
            assert_eq!(
                prepared.call.starts_detached,
                spec.starts_detached_for(&input)
            );
            assert!(prepared.auto_approve);
        }
    }

    #[test]
    fn hook_rewrite_discards_every_original_prepared_decision() {
        let (_root, registry) = registry();
        let original = prepare_tool_call(
            "input_dependent",
            json!({"safe": true, "detached": false}),
            Some(&registry),
            false,
        )
        .expect("prepare original");
        let rewritten = reprepare_tool_call_after_hook(
            "input_dependent",
            json!({"safe": false, "detached": true}),
            Some(&registry),
            false,
        )
        .expect("reprepare rewritten input");

        assert_eq!(original.call.approval, ApprovalRequirement::Auto);
        assert!(original.call.read_only);
        assert!(original.call.supports_parallel);
        assert!(!original.call.starts_detached);

        assert_eq!(rewritten.call.approval, ApprovalRequirement::Required);
        assert!(!rewritten.call.read_only);
        assert!(!rewritten.call.supports_parallel);
        assert!(rewritten.call.starts_detached);
        assert_eq!(
            rewritten.call.input,
            json!({"safe": false, "detached": true})
        );
    }

    #[test]
    fn bypass_preparation_preserves_legacy_policy_table() {
        struct Expected {
            name: &'static str,
            approval: ApprovalRequirement,
            read_only: bool,
            supports_parallel: bool,
            global_exclusive: bool,
        }

        for expected in [
            Expected {
                name: "read_mcp_resource",
                approval: ApprovalRequirement::Auto,
                read_only: true,
                supports_parallel: true,
                global_exclusive: true,
            },
            Expected {
                name: "mcp_filesystem_write",
                approval: ApprovalRequirement::Suggest,
                read_only: false,
                supports_parallel: false,
                global_exclusive: true,
            },
            Expected {
                name: CODE_EXECUTION_TOOL_NAME,
                approval: ApprovalRequirement::Suggest,
                read_only: false,
                supports_parallel: false,
                global_exclusive: true,
            },
            Expected {
                name: JS_EXECUTION_TOOL_NAME,
                approval: ApprovalRequirement::Suggest,
                read_only: false,
                supports_parallel: false,
                global_exclusive: true,
            },
            Expected {
                name: "tool_search",
                approval: ApprovalRequirement::Auto,
                read_only: true,
                supports_parallel: false,
                global_exclusive: false,
            },
        ] {
            let prepared = prepare_tool_call(expected.name, json!({}), None, false)
                .unwrap_or_else(|error| panic!("prepare {}: {error}", expected.name));
            assert_eq!(
                prepared.call.approval, expected.approval,
                "{}",
                expected.name
            );
            assert_eq!(
                prepared.call.read_only, expected.read_only,
                "{}",
                expected.name
            );
            assert_eq!(
                prepared.call.supports_parallel, expected.supports_parallel,
                "{}",
                expected.name
            );
            assert_eq!(
                prepared.call.resources == vec![ResourceClaim::GlobalExclusive],
                expected.global_exclusive,
                "{}",
                expected.name
            );
            assert!(!prepared.call.starts_detached, "{}", expected.name);
            assert!(!prepared.auto_approve, "{}", expected.name);
        }
    }

    #[test]
    fn mcp_write_preparation_respects_session_auto_approval() {
        let prepared = prepare_tool_call("mcp_filesystem_write", json!({}), None, true)
            .expect("prepare MCP write tool with session auto-approval");

        assert_eq!(prepared.call.approval, ApprovalRequirement::Suggest);
        assert!(!prepared.call.read_only);
        assert!(!prepared.call.supports_parallel);
        assert_eq!(
            prepared.call.resources,
            vec![ResourceClaim::GlobalExclusive]
        );
        assert!(prepared.auto_approve);
        assert!(!super::super::turn_loop::registered_tool_approval_required(
            &prepared.call.name,
            prepared.call.approval,
            prepared.auto_approve,
        ));
    }

    #[test]
    fn hook_rewrite_reprepares_resource_claims_from_final_input() {
        let root = tempdir().expect("tempdir");
        let context = ToolContext::new(root.path().to_path_buf());
        let original_path = context.resolve_path("before.rs").expect("original path");
        let rewritten_path = context.resolve_path("after.rs").expect("rewritten path");
        let mut registry = ToolRegistry::new(context);
        registry.register(Arc::new(crate::tools::file::ReadFileTool));

        let original = prepare_tool_call(
            "read_file",
            json!({"path": "before.rs"}),
            Some(&registry),
            false,
        )
        .expect("prepare original read");
        let rewritten = reprepare_tool_call_after_hook(
            "read_file",
            json!({"path": "after.rs"}),
            Some(&registry),
            false,
        )
        .expect("reprepare rewritten read");

        assert_eq!(
            original.call.resources,
            vec![ResourceClaim::ReadPath(original_path)]
        );
        assert_eq!(
            rewritten.call.resources,
            vec![ResourceClaim::ReadPath(rewritten_path)]
        );
    }

    #[test]
    fn registered_file_claims_are_canonical_and_input_specific() {
        let root = tempdir().expect("tempdir");
        let context = ToolContext::new(root.path().to_path_buf());
        let exact = context.resolve_path("src/lib.rs").expect("exact path");
        let tree = context.resolve_path("src").expect("tree path");

        assert_eq!(
            registered_resource_claims("read_file", &json!({"path": "src/lib.rs"}), &context,)
                .expect("read claim"),
            vec![ResourceClaim::ReadPath(exact.clone())]
        );
        assert_eq!(
            registered_resource_claims("edit_file", &json!({"path": "src/lib.rs"}), &context,)
                .expect("write claim"),
            vec![ResourceClaim::WritePath(exact)]
        );
        assert_eq!(
            registered_resource_claims("grep_files", &json!({"path": "src"}), &context)
                .expect("tree claim"),
            vec![ResourceClaim::ReadTree(tree)]
        );
        assert_eq!(
            registered_resource_claims("read_file", &json!({"path": "../../outside"}), &context,)
                .expect("path escape must fall back conservatively"),
            vec![ResourceClaim::GlobalExclusive]
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_aliases_resolve_to_the_same_file_claim() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("tempdir");
        let real_dir = root.path().join("real");
        std::fs::create_dir(&real_dir).expect("create real directory");
        std::fs::write(real_dir.join("lib.rs"), "fn main() {}\n").expect("write real file");
        symlink("real", root.path().join("alias")).expect("create directory symlink");
        let context = ToolContext::new(root.path().to_path_buf());
        let canonical_real = real_dir
            .join("lib.rs")
            .canonicalize()
            .expect("canonical file");

        let read_alias =
            registered_resource_claims("read_file", &json!({"path": "alias/lib.rs"}), &context)
                .expect("alias claim");
        let write_real =
            registered_resource_claims("write_file", &json!({"path": "real/lib.rs"}), &context)
                .expect("real claim");

        assert_eq!(read_alias, vec![ResourceClaim::ReadPath(canonical_real)]);
        assert!(read_alias[0].conflicts_with(&write_real[0]));
    }

    #[test]
    fn apply_patch_claims_every_resolved_target_or_falls_back_global() {
        let root = tempdir().expect("tempdir");
        let context = ToolContext::new(root.path().to_path_buf());
        let a = context.resolve_path("a.rs").expect("a path");
        let b = context.resolve_path("b.rs").expect("b path");

        let claims = registered_resource_claims(
            "apply_patch",
            &json!({
                "replace": [
                    {"path": "b.rs", "content": "b"},
                    {"path": "a.rs", "content": "a"}
                ]
            }),
            &context,
        )
        .expect("patch claims");
        assert_eq!(
            claims,
            vec![ResourceClaim::WritePath(a), ResourceClaim::WritePath(b)]
        );

        assert_eq!(
            registered_resource_claims(
                "apply_patch",
                &json!({"patch": "not a unified diff"}),
                &context,
            )
            .expect("fallback claim"),
            vec![ResourceClaim::GlobalExclusive]
        );
        assert_eq!(
            registered_resource_claims(
                "apply_patch",
                &json!({"replace": [{"path": "../../outside", "content": "nope"}]}),
                &context,
            )
            .expect("escaped target fallback"),
            vec![ResourceClaim::GlobalExclusive]
        );
    }

    #[test]
    fn terminal_and_unknown_tools_keep_conservative_claims() {
        let root = tempdir().expect("tempdir");
        let context = ToolContext::new(root.path().to_path_buf());

        assert_eq!(
            registered_resource_claims("terminal/run", &json!({}), &context)
                .expect("default terminal"),
            vec![ResourceClaim::Terminal("term-1".to_string())]
        );
        assert_eq!(
            registered_resource_claims(
                "exec_shell_interact",
                &json!({"task_id": "task-7"}),
                &context,
            )
            .expect("task terminal"),
            vec![ResourceClaim::Terminal("task-7".to_string())]
        );
        assert_eq!(
            registered_resource_claims("plugin_tool", &json!({}), &context).expect("unknown tool"),
            vec![ResourceClaim::GlobalExclusive]
        );
    }
}
