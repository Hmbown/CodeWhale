//! Model-facing LSP code-intelligence tool.
//!
//! Extends the existing [`crate::lsp::LspManager`] lifecycle — never spawns a
//! competing server pool. Operations: diagnostics, read_lints, symbols,
//! definition, references.

use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_str, required_str,
};

/// Model-callable LSP intelligence surface.
pub struct LspTool;

#[async_trait]
impl ToolSpec for LspTool {
    fn name(&self) -> &'static str {
        "lsp"
    }

    fn description(&self) -> &'static str {
        "Query language-server intelligence for a file: diagnostics, document \
         or workspace symbols, go-to-definition, and find-references. Reuses \
         the session LSP manager (no separate server lifecycle). Requires \
         `[lsp] enabled = true` and a configured server for the file language."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["diagnostics", "read_lints", "symbols", "definition", "references"],
                    "description": "Operation to run."
                },
                "path": {
                    "type": "string",
                    "description": "Workspace-relative or absolute source file path. For read_lints, pass newline-separated workspace-relative paths."
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line."
                },
                "character": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based column, default 1."
                },
                "query": {
                    "type": "string",
                    "description": "Workspace symbol query when operation=symbols."
                }
            },
            "required": ["operation", "path"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let operation = required_str(&input, "operation")?;
        let path_raw = required_str(&input, "path")?;
        let line = input.get("line").and_then(|v| v.as_u64()).map(|n| n as u32);
        let character = input
            .get("character")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let query = optional_str(&input, "query")?;

        if operation == "read_lints" {
            let paths = path_raw
                .split('\n')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            return execute_read_lints(json!({"paths": paths}), context).await;
        }

        let manager = context.lsp_manager.as_ref().ok_or_else(|| {
            ToolError::execution_failed(
                "LSP manager is not attached to this tool context (LSP unavailable for this session)",
            )
        })?;

        let path = resolve_workspace_path(&context.workspace, path_raw);
        let payload = manager
            .intelligence(operation, &path, line, character, query)
            .await
            .map_err(ToolError::execution_failed)?;

        Ok(ToolResult::success(
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()),
        ))
    }
}

const MAX_LINT_PATHS: usize = 16;
const MAX_LINT_DIAGNOSTICS: usize = 100;
const MAX_LINT_MESSAGE_CHARS: usize = 512;
const MAX_LINT_OUTPUT_CHARS: usize = 12_000;

/// Read bounded diagnostics for several existing files without requiring a
/// preceding edit. The model-facing entry point is the `lsp` operation above;
/// keeping this as a helper avoids adding a second catalog tool name.
async fn execute_read_lints(input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
    let raw_paths = input
        .get("paths")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolError::invalid_input("paths must be a non-empty array"))?;
    if raw_paths.is_empty() || raw_paths.len() > MAX_LINT_PATHS {
        return Err(ToolError::invalid_input(format!(
            "paths must contain between 1 and {MAX_LINT_PATHS} files"
        )));
    }

    let paths = raw_paths
        .iter()
        .map(|value| {
            let raw = value
                .as_str()
                .ok_or_else(|| ToolError::invalid_input("each paths entry must be a string"))?;
            resolve_lint_path(&context.workspace, raw)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let manager = context.lsp_manager.as_ref().ok_or_else(|| {
        ToolError::execution_failed(
            "LSP manager is not attached to this tool context; enable LSP for this session",
        )
    })?;
    let (include_warnings, reports) = manager
        .diagnostics_for_paths(&paths)
        .await
        .map_err(ToolError::execution_failed)?;

    let mut files = Vec::with_capacity(reports.len());
    let mut diagnostic_count = 0usize;
    let mut truncated = false;
    for report in reports {
        let file_display = report.block.file.display().to_string();
        let mut items = Vec::new();
        let mut file_truncated = report.truncated;
        if report.unavailable.is_none() {
            for diagnostic in report.block.items {
                if diagnostic_count >= MAX_LINT_DIAGNOSTICS {
                    truncated = true;
                    file_truncated = true;
                    break;
                }
                diagnostic_count += 1;
                items.push(json!({
                    "line": diagnostic.line,
                    "column": diagnostic.column,
                    "severity": format!("{:?}", diagnostic.severity).to_ascii_lowercase(),
                    "message": diagnostic
                        .message
                        .chars()
                        .take(MAX_LINT_MESSAGE_CHARS)
                        .collect::<String>(),
                }));
            }
        }
        let mut entry = json!({
            "file": file_display,
            "status": if report.unavailable.is_some() {
                "unavailable"
            } else if items.is_empty() {
                "clean"
            } else {
                "ok"
            },
            "diagnostics": items,
        });
        if let Some(note) = report.unavailable {
            entry["note"] = json!(note);
        }
        if file_truncated {
            entry["truncated"] = Value::Bool(true);
        }
        files.push(entry);
    }

    let mut output = json!({
        "files": files,
        "diagnostic_count": diagnostic_count,
        "truncated": truncated,
        "warnings_included": include_warnings,
    });
    while serde_json::to_string(&output)
        .map(|value| value.len() > MAX_LINT_OUTPUT_CHARS)
        .unwrap_or(false)
    {
        let Some(files) = output.get_mut("files").and_then(Value::as_array_mut) else {
            break;
        };
        let Some(last) = files.last_mut() else {
            break;
        };
        if let Some(items) = last.get_mut("diagnostics").and_then(Value::as_array_mut)
            && items.pop().is_some()
        {
            output["truncated"] = Value::Bool(true);
        } else {
            files.pop();
            output["truncated"] = Value::Bool(true);
        }
    }

    ToolResult::json(&output).map_err(|error| ToolError::execution_failed(error.to_string()))
}

fn resolve_lint_path(workspace: &Path, raw: &str) -> Result<PathBuf, ToolError> {
    let raw = raw.trim();
    let candidate = Path::new(raw);
    if raw.is_empty() || candidate.is_absolute() {
        return Err(ToolError::permission_denied(
            "read_lints paths must be non-empty workspace-relative files",
        ));
    }
    if candidate
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(ToolError::permission_denied(
            "read_lints paths cannot contain '..' traversal",
        ));
    }
    let workspace = workspace.canonicalize().map_err(|error| {
        ToolError::execution_failed(format!("failed to resolve workspace: {error}"))
    })?;
    let path = workspace.join(candidate).canonicalize().map_err(|error| {
        ToolError::execution_failed(format!("failed to read_lints path {raw}: {error}"))
    })?;
    if !path.starts_with(&workspace) {
        return Err(ToolError::permission_denied(
            "read_lints path resolves outside the workspace",
        ));
    }
    if !path.is_file() {
        return Err(ToolError::invalid_input(format!(
            "read_lints path is not a file: {raw}"
        )));
    }
    Ok(path)
}

fn resolve_workspace_path(workspace: &std::path::Path, raw: &str) -> std::path::PathBuf {
    let candidate = std::path::PathBuf::from(raw);
    if candidate.is_absolute() {
        candidate
    } else {
        workspace.join(candidate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::{Diagnostic, Language, LspConfig, LspManager, Severity};
    use crate::tools::spec::ToolContext;
    use async_trait::async_trait;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;

    struct CountingTransport {
        calls: AtomicUsize,
        request_calls: AtomicUsize,
    }

    #[async_trait]
    impl crate::lsp::LspTransport for CountingTransport {
        async fn diagnostics_for(
            &self,
            _path: &Path,
            _text: &str,
            _wait: Duration,
        ) -> anyhow::Result<Vec<Diagnostic>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(vec![Diagnostic {
                line: 1,
                column: 1,
                severity: Severity::Error,
                message: "boom".into(),
            }])
        }

        async fn request(
            &self,
            method: &str,
            _params: Value,
            _wait: Duration,
        ) -> anyhow::Result<Value> {
            self.request_calls.fetch_add(1, Ordering::Relaxed);
            Ok(json!({ "method": method, "locations": [] }))
        }

        async fn shutdown(&self) {}
    }

    struct EmptyTransport;

    #[async_trait]
    impl crate::lsp::LspTransport for EmptyTransport {
        async fn diagnostics_for(
            &self,
            _path: &Path,
            _text: &str,
            _wait: Duration,
        ) -> anyhow::Result<Vec<Diagnostic>> {
            Ok(Vec::new())
        }

        async fn request(
            &self,
            _method: &str,
            _params: Value,
            _wait: Duration,
        ) -> anyhow::Result<Value> {
            Ok(json!({}))
        }

        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn tool_reuses_single_manager_transport_for_definition() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        let transport = Arc::new(CountingTransport {
            calls: AtomicUsize::new(0),
            request_calls: AtomicUsize::new(0),
        });
        mgr.install_test_transport(Language::Rust, transport.clone())
            .await;

        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let tool = LspTool;
        for _ in 0..2 {
            let result = tool
                .execute(
                    json!({
                        "operation": "definition",
                        "path": "lib.rs",
                        "line": 1,
                        "character": 4
                    }),
                    &ctx,
                )
                .await
                .expect("definition succeeds");
            assert!(result.success, "{}", result.content);
            assert!(result.content.contains("definition"));
        }
        assert_eq!(
            transport.request_calls.load(Ordering::Relaxed),
            2,
            "two definition calls"
        );
    }

    #[tokio::test]
    async fn diagnostics_operation_returns_items() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        let transport = Arc::new(CountingTransport {
            calls: AtomicUsize::new(0),
            request_calls: AtomicUsize::new(0),
        });
        mgr.install_test_transport(Language::Rust, transport.clone())
            .await;

        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(
                json!({ "operation": "diagnostics", "path": "lib.rs" }),
                &ctx,
            )
            .await
            .expect("diagnostics");
        assert!(result.success);
        assert!(result.content.contains("boom"));
        assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn read_lints_returns_structured_diagnostics_for_multiple_files() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("lib.rs");
        let second = dir.path().join("main.rs");
        tokio::fs::write(&first, b"fn lib() {}\n").await.unwrap();
        tokio::fs::write(&second, b"fn main() {}\n").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        mgr.install_test_transport(
            Language::Rust,
            Arc::new(CountingTransport {
                calls: AtomicUsize::new(0),
                request_calls: AtomicUsize::new(0),
            }),
        )
        .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(
                json!({
                    "operation": "read_lints",
                    "path": "lib.rs\nmain.rs"
                }),
                &ctx,
            )
            .await
            .expect("read_lints");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["files"].as_array().unwrap().len(), 2);
        assert_eq!(payload["diagnostic_count"], 2);
        assert_eq!(payload["files"][0]["status"], "ok");
        assert_eq!(payload["warnings_included"], false);
        assert_eq!(payload["files"][0]["diagnostics"][0]["line"], 1);
        assert_eq!(payload["files"][0]["diagnostics"][0]["severity"], "error");
        assert_eq!(payload["files"][0]["diagnostics"][0]["message"], "boom");
    }

    #[tokio::test]
    async fn read_lints_preserves_files_with_empty_diagnostics() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}\n").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        mgr.install_test_transport(Language::Rust, Arc::new(EmptyTransport))
            .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("empty diagnostics are a successful read");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["diagnostic_count"], 0);
        assert_eq!(payload["files"][0]["diagnostics"], json!([]));
        assert_eq!(payload["files"][0]["status"], "clean");
    }

    #[tokio::test]
    async fn disabled_lsp_hard_blocks_tool() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}").await.unwrap();
        let mgr = Arc::new(LspManager::new(
            LspConfig {
                enabled: false,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        ));
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);
        let err = LspTool
            .execute(
                json!({ "operation": "diagnostics", "path": "lib.rs" }),
                &ctx,
            )
            .await
            .expect_err("disabled must fail");
        assert!(
            err.to_string().contains("disabled"),
            "unexpected error: {err}"
        );

        let path_error = LspTool
            .execute(
                json!({"operation": "read_lints", "path": "../outside.rs"}),
                &ctx,
            )
            .await
            .expect_err("path traversal must fail closed");
        assert!(path_error.to_string().contains("cannot contain"));
    }

    struct StubTransport {
        diagnostics: Vec<Diagnostic>,
        fail: bool,
        stall: Duration,
    }

    #[async_trait]
    impl crate::lsp::LspTransport for StubTransport {
        async fn diagnostics_for(
            &self,
            _path: &Path,
            _text: &str,
            _wait: Duration,
        ) -> anyhow::Result<Vec<Diagnostic>> {
            if !self.stall.is_zero() {
                tokio::time::sleep(self.stall).await;
            }
            if self.fail {
                return Err(anyhow::anyhow!("kaboom"));
            }
            Ok(self.diagnostics.clone())
        }

        async fn request(
            &self,
            _method: &str,
            _params: Value,
            _wait: Duration,
        ) -> anyhow::Result<Value> {
            Ok(json!({}))
        }

        async fn shutdown(&self) {}
    }

    #[tokio::test]
    async fn read_lints_reports_timeout_as_unavailable_not_clean() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}\n").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig {
                poll_after_edit_ms: 40,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        ));
        mgr.install_test_transport(
            Language::Rust,
            Arc::new(StubTransport {
                diagnostics: Vec::new(),
                fail: false,
                stall: Duration::from_millis(250),
            }),
        )
        .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("degraded polls still succeed with a visible note");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["files"][0]["status"], "unavailable");
        assert!(
            payload["files"][0]["note"]
                .as_str()
                .is_some_and(|note| note.contains("timed out")),
            "unexpected note: {payload}"
        );
        assert_eq!(payload["diagnostic_count"], 0);
    }

    #[tokio::test]
    async fn read_lints_reports_transport_errors_as_unavailable() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}\n").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        mgr.install_test_transport(
            Language::Rust,
            Arc::new(StubTransport {
                diagnostics: Vec::new(),
                fail: true,
                stall: Duration::ZERO,
            }),
        )
        .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("transport errors surface as unavailable, not clean");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["files"][0]["status"], "unavailable");
        assert!(
            payload["files"][0]["note"]
                .as_str()
                .is_some_and(|note| note.contains("kaboom")),
            "unexpected note: {payload}"
        );
    }

    #[tokio::test]
    async fn read_lints_flags_config_cap_truncation() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}\n").await.unwrap();

        let mgr = Arc::new(LspManager::new(
            LspConfig {
                max_diagnostics_per_file: 2,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        ));
        mgr.install_test_transport(
            Language::Rust,
            Arc::new(StubTransport {
                diagnostics: (1..=3)
                    .map(|line| Diagnostic {
                        line,
                        column: 1,
                        severity: Severity::Error,
                        message: format!("err {line}"),
                    })
                    .collect(),
                fail: false,
                stall: Duration::ZERO,
            }),
        )
        .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(mgr);

        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("capped lists stay readable");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(
            payload["files"][0]["diagnostics"].as_array().unwrap().len(),
            2
        );
        assert_eq!(payload["files"][0]["truncated"], true);
        assert_eq!(payload["diagnostic_count"], 2);
        assert_eq!(payload["truncated"], false, "tool caps were not reached");
    }

    #[tokio::test]
    async fn read_lints_surfaces_warning_visibility() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("lib.rs");
        tokio::fs::write(&path, b"fn main() {}\n").await.unwrap();
        let stub = || -> Arc<StubTransport> {
            Arc::new(StubTransport {
                diagnostics: vec![
                    Diagnostic {
                        line: 1,
                        column: 1,
                        severity: Severity::Error,
                        message: "e1".into(),
                    },
                    Diagnostic {
                        line: 2,
                        column: 1,
                        severity: Severity::Warning,
                        message: "w1".into(),
                    },
                ],
                fail: false,
                stall: Duration::ZERO,
            })
        };

        let errors_only = Arc::new(LspManager::new(
            LspConfig::default(),
            dir.path().to_path_buf(),
        ));
        errors_only
            .install_test_transport(Language::Rust, stub())
            .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(Arc::clone(&errors_only));
        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("default filter read");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["warnings_included"], false);
        assert_eq!(
            payload["files"][0]["diagnostics"].as_array().unwrap().len(),
            1
        );
        assert_eq!(payload["files"][0]["diagnostics"][0]["severity"], "error");

        let with_warnings = Arc::new(LspManager::new(
            LspConfig {
                include_warnings: true,
                ..LspConfig::default()
            },
            dir.path().to_path_buf(),
        ));
        with_warnings
            .install_test_transport(Language::Rust, stub())
            .await;
        let mut ctx = ToolContext::new(dir.path());
        ctx = ctx.with_lsp_manager(with_warnings);
        let result = LspTool
            .execute(json!({"operation": "read_lints", "path": "lib.rs"}), &ctx)
            .await
            .expect("warnings-included read");
        let payload: Value = serde_json::from_str(&result.content).unwrap();
        assert_eq!(payload["warnings_included"], true);
        assert_eq!(
            payload["files"][0]["diagnostics"].as_array().unwrap().len(),
            2
        );
    }
}
