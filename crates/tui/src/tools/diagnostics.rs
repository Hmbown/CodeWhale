//! Workspace diagnostics tool: `diagnostics`.
//!
//! This tool gathers lightweight, best-effort environment information without
//! failing hard when optional commands are unavailable.

use std::env;
use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

/// Tool for collecting workspace and toolchain diagnostics.
pub struct DiagnosticsTool;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DiagnosticsOutput {
    workspace_root: String,
    current_dir: Option<String>,
    current_dir_error: Option<String>,
    git_repo: bool,
    git_branch: Option<String>,
    git_error: Option<String>,
    sandbox_available: bool,
    sandbox_type: Option<String>,
    rustc_version: Option<String>,
    cargo_version: Option<String>,
    /// User-trusted external paths the agent may access from this workspace
    /// (`/trust add <path>` from the slash command, persisted in
    /// `~/.deepseek/workspace-trust.json`). See issue #29.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    trusted_external_paths: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct GitProbe {
    detected: bool,
    branch: Option<String>,
    error: Option<String>,
}

#[async_trait]
impl ToolSpec for DiagnosticsTool {
    fn name(&self) -> &'static str {
        "diagnostics"
    }

    fn description(&self) -> &'static str {
        "Report workspace info, git detection, sandbox availability, and Rust toolchain versions."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    fn supports_parallel(&self) -> bool {
        true
    }

    async fn execute(&self, _input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let workspace_root = context.workspace.display().to_string();

        let (current_dir, current_dir_error) = match env::current_dir() {
            Ok(dir) => (Some(dir.display().to_string()), None),
            Err(err) => (None, Some(err.to_string())),
        };

        let git = probe_git(&context.workspace);
        let sandbox_type = crate::sandbox::get_platform_sandbox().map(|s| s.to_string());
        let sandbox_available = sandbox_type.is_some();

        let trusted_external_paths = context
            .trusted_external_paths
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        let diagnostics = DiagnosticsOutput {
            workspace_root,
            current_dir,
            current_dir_error,
            git_repo: git.detected,
            git_branch: git.branch,
            git_error: git.error,
            sandbox_available,
            sandbox_type,
            rustc_version: probe_version("rustc", &["--version"], &context.workspace),
            cargo_version: probe_version("cargo", &["--version"], &context.workspace),
            trusted_external_paths,
        };

        ToolResult::json(&diagnostics).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

// === Helpers ===

fn probe_git(workspace: &Path) -> GitProbe {
    let rev_parse = run_command("git", &["rev-parse", "--is-inside-work-tree"], workspace);
    match rev_parse {
        CommandProbe::Success(out) => {
            if out.trim() != "true" {
                return GitProbe {
                    detected: false,
                    branch: None,
                    error: Some(format!("unexpected git rev-parse output: {out}")),
                };
            }
            let branch = run_command("git", &["rev-parse", "--abbrev-ref", "HEAD"], workspace)
                .into_success();
            GitProbe {
                detected: true,
                branch,
                error: None,
            }
        }
        CommandProbe::Failed { stderr, .. } => GitProbe {
            detected: false,
            branch: None,
            error: stderr,
        },
        CommandProbe::Missing => GitProbe {
            detected: false,
            branch: None,
            error: Some("git is not installed or not in PATH".to_string()),
        },
    }
}

fn probe_version(program: &str, args: &[&str], cwd: &Path) -> Option<String> {
    run_command(program, args, cwd).into_success()
}

enum CommandProbe {
    Success(String),
    Failed { stderr: Option<String> },
    Missing,
}

impl CommandProbe {
    fn into_success(self) -> Option<String> {
        match self {
            CommandProbe::Success(out) => Some(out),
            CommandProbe::Failed { .. } | CommandProbe::Missing => None,
        }
    }
}

fn run_command(program: &str, args: &[&str], cwd: &Path) -> CommandProbe {
    let output = Command::new(program).args(args).current_dir(cwd).output();
    let output = match output {
        Ok(output) => output,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return CommandProbe::Missing,
        Err(_) => return CommandProbe::Failed { stderr: None },
    };

    if output.status.success() {
        CommandProbe::Success(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        CommandProbe::Failed {
            stderr: if stderr.is_empty() {
                None
            } else {
                Some(stderr)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;

    fn git_available() -> bool {
        Command::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn init_git_repo(root: &Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(root)
                .status()
                .expect("git should spawn");
            assert!(status.success(), "git {:?} failed", args);
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Test User"]);
        fs::write(root.join("README.md"), "init\n").expect("write");
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn diagnostics_schema_has_empty_required_array() {
        let schema = DiagnosticsTool.input_schema();
        assert_eq!(schema["properties"], json!({}));
        assert_eq!(schema["required"], json!([]));
    }

    #[tokio::test]
    async fn diagnostics_runs_best_effort_outside_git_repo() {
        let tmp = tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path());
        let tool = DiagnosticsTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);

        let parsed: DiagnosticsOutput =
            serde_json::from_str(&result.content).expect("tool result should be json");
        assert_eq!(parsed.workspace_root, tmp.path().display().to_string());
    }

    #[tokio::test]
    async fn diagnostics_detects_git_repo_when_available() {
        if !git_available() {
            return;
        }
        let tmp = tempdir().expect("tempdir");
        init_git_repo(tmp.path());

        let ctx = ToolContext::new(tmp.path());
        let tool = DiagnosticsTool;
        let result = tool.execute(json!({}), &ctx).await.expect("execute");
        assert!(result.success);

        let parsed: DiagnosticsOutput =
            serde_json::from_str(&result.content).expect("tool result should be json");
        assert!(parsed.git_repo);
        assert!(!parsed.git_branch.as_deref().unwrap_or("").is_empty());
    }
}
