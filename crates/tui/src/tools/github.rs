//! GitHub context and guarded write tools backed by the `gh` CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::task_manager::{TaskArtifactRef, TaskGithubEvent};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, required_str, required_u64,
};

const DEFAULT_GH: &str = "/opt/homebrew/bin/gh";
const FALLBACK_GH_PATHS: &[&str] = &[
    "/usr/bin/gh",                       // Linux system package manager
    "/usr/local/bin/gh",                 // macOS Intel Homebrew / manual install
    "/home/linuxbrew/.linuxbrew/bin/gh", // Linux Homebrew (official prefix)
    "/opt/homebrew/bin/gh",              // macOS Apple Silicon Homebrew
];
const BODY_ARTIFACT_THRESHOLD: usize = 4_000;
const DIFF_ARTIFACT_THRESHOLD: usize = 8_000;

pub struct GithubIssueContextTool;
pub struct GithubPrContextTool;
pub struct GithubCommentTool;
pub struct GithubCloseIssueTool;

#[async_trait]
impl ToolSpec for GithubIssueContextTool {
    fn name(&self) -> &'static str {
        "github_issue_context"
    }

    fn description(&self) -> &'static str {
        "Read GitHub issue context using gh. Read-only: body/comments/labels/state are summarized and large bodies become task artifacts when a durable task is active."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "number": { "type": "integer", "minimum": 1 },
                "include_comments": { "type": "boolean", "default": true }
            },
            "required": ["number"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        ensure_github_repo(context)?;
        let number = required_u64(&input, "number")?;
        let include_comments = optional_bool(&input, "include_comments", true);
        let fields = if include_comments {
            "number,title,state,author,labels,assignees,milestone,body,comments,url,createdAt,updatedAt"
        } else {
            "number,title,state,author,labels,assignees,milestone,body,url,createdAt,updatedAt"
        };
        let number_s = number.to_string();
        let raw = run_gh_json(context, &["issue", "view", &number_s, "--json", fields])?;
        let shaped = shape_large_text(context, raw, "issue_body", BODY_ARTIFACT_THRESHOLD)?;
        let mut result = ToolResult::json(&json!({
            "summary": format!("Issue #{number}: {}", shaped["title"].as_str().unwrap_or("")),
            "issue": shaped,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let artifacts = artifact_refs_from_context(&result.content, "github_issue_body");
        if !artifacts.is_empty() {
            result = result.with_metadata(json!({ "task_updates": { "artifacts": artifacts } }));
        }
        Ok(result)
    }
}

#[async_trait]
impl ToolSpec for GithubPrContextTool {
    fn name(&self) -> &'static str {
        "github_pr_context"
    }

    fn description(&self) -> &'static str {
        "Read GitHub PR context using gh: body/comments/reviews/check status/files and optional diff artifact. Read-only; no push/merge/close."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "number": { "type": "integer", "minimum": 1 },
                "include_diff": { "type": "boolean", "default": false }
            },
            "required": ["number"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly, ToolCapability::Network]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        ensure_github_repo(context)?;
        let number = required_u64(&input, "number")?;
        let number_s = number.to_string();
        let raw = run_gh_json(
            context,
            &[
                "pr",
                "view",
                &number_s,
                "--json",
                "number,title,state,author,body,comments,reviews,reviewDecision,statusCheckRollup,baseRefName,headRefName,headRefOid,baseRefOid,files,url,createdAt,updatedAt",
            ],
        )?;
        let mut shaped = shape_large_text(context, raw, "pr_body", BODY_ARTIFACT_THRESHOLD)?;
        if optional_bool(&input, "include_diff", false) {
            let diff = run_gh_text(context, &["pr", "diff", &number_s, "--patch"])?;
            let diff_ref =
                write_artifact_if_needed(context, "pr_diff", &diff, DIFF_ARTIFACT_THRESHOLD)?;
            shaped["diff_summary"] = json!(summarize(&diff, 900));
            shaped["diff_artifact"] = json!(diff_ref);
        }
        let mut result = ToolResult::json(&json!({
            "summary": format!("PR #{number}: {}", shaped["title"].as_str().unwrap_or("")),
            "pr": shaped,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let mut artifacts = artifact_refs_from_context(&result.content, "github_pr_body");
        artifacts.extend(artifact_refs_from_context(
            &result.content,
            "github_pr_diff",
        ));
        if !artifacts.is_empty() {
            result = result.with_metadata(json!({ "task_updates": { "artifacts": artifacts } }));
        }
        Ok(result)
    }
}

#[async_trait]
impl ToolSpec for GithubCommentTool {
    fn name(&self) -> &'static str {
        "github_comment"
    }

    fn description(&self) -> &'static str {
        "Post an evidence-backed GitHub issue/PR comment with gh. Requires approval. Use blocker comments for partial work; do not claim closure without evidence."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target": { "type": "string", "enum": ["issue", "pr"] },
                "number": { "type": "integer", "minimum": 1 },
                "body": { "type": "string" },
                "evidence": { "type": "object" },
                "dry_run": { "type": "boolean", "default": false }
            },
            "required": ["target", "number", "body", "evidence"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network, ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        validate_evidence(&input, false)?;
        let target = required_str(&input, "target")?;
        let number = required_u64(&input, "number")?;
        let body = required_str(&input, "body")?;
        if optional_bool(&input, "dry_run", false) {
            return Ok(ToolResult::success(format!(
                "Dry run: would comment on {target} #{number}."
            )));
        }
        let subcmd = if target == "pr" { "pr" } else { "issue" };
        let number_s = number.to_string();
        run_gh_text(context, &[subcmd, "comment", &number_s, "--body", body])?;
        let metadata = github_event_metadata(
            "comment",
            target,
            number,
            summarize(body, 240),
            None,
            write_artifact_if_needed(context, "github_comment", body, BODY_ARTIFACT_THRESHOLD)?,
        );
        Ok(
            ToolResult::success(format!("Commented on {target} #{number}."))
                .with_metadata(metadata),
        )
    }
}

#[async_trait]
impl ToolSpec for GithubCloseIssueTool {
    fn name(&self) -> &'static str {
        "github_close_issue"
    }

    fn description(&self) -> &'static str {
        "Close a GitHub issue only when structured acceptance evidence is present and approved. Never close merely because the agent is stopping."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "number": { "type": "integer", "minimum": 1 },
                "acceptance_criteria": { "type": "array", "items": { "type": "string" }, "minItems": 1 },
                "evidence": {
                    "type": "object",
                    "properties": {
                        "files_changed": { "type": "array", "items": { "type": "string" } },
                        "tests_run": { "type": "array", "items": { "type": "string" } },
                        "commits": { "type": "array", "items": { "type": "string" } },
                        "final_status": { "type": "string" }
                    },
                    "required": ["files_changed", "tests_run", "final_status"]
                },
                "comment": { "type": "string" },
                "allow_dirty": { "type": "boolean", "default": false },
                "dry_run": { "type": "boolean", "default": false }
            },
            "required": ["number", "acceptance_criteria", "evidence"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network, ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        validate_evidence(&input, true)?;
        if !optional_bool(&input, "allow_dirty", false) {
            let status = git_status_porcelain(context)?;
            if !status.trim().is_empty() {
                return Ok(ToolResult::error(
                    "Refusing to close issue: worktree is dirty and allow_dirty was false.",
                )
                .with_metadata(json!({ "dirty_status": status })));
            }
        }
        let number = required_u64(&input, "number")?;
        if optional_bool(&input, "dry_run", false) {
            return Ok(ToolResult::success(format!(
                "Dry run: would close issue #{number}."
            )));
        }
        if let Some(comment) = optional_str(&input, "comment") {
            let number_s = number.to_string();
            run_gh_text(context, &["issue", "comment", &number_s, "--body", comment])?;
        }
        let number_s = number.to_string();
        run_gh_text(
            context,
            &["issue", "close", &number_s, "--reason", "completed"],
        )?;
        let metadata = github_event_metadata(
            "close",
            "issue",
            number,
            "Issue closed as completed with structured evidence".to_string(),
            None,
            optional_str(&input, "comment")
                .and_then(|comment| {
                    write_artifact_if_needed(
                        context,
                        "github_close_comment",
                        comment,
                        BODY_ARTIFACT_THRESHOLD,
                    )
                    .ok()
                })
                .flatten(),
        );
        Ok(ToolResult::success(format!("Closed issue #{number}.")).with_metadata(metadata))
    }
}

fn gh_bin() -> String {
    if let Ok(bin) = std::env::var("DEEPSEEK_GH_BIN") {
        return bin;
    }
    for path in FALLBACK_GH_PATHS {
        if std::path::Path::new(path).is_file() {
            return path.to_string();
        }
    }
    DEFAULT_GH.to_string()
}

fn run_gh_text(context: &ToolContext, args: &[&str]) -> Result<String, ToolError> {
    let out = Command::new(gh_bin())
        .args(args)
        .current_dir(&context.workspace)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ToolError::not_available("gh CLI not found; install it or set DEEPSEEK_GH_BIN")
            } else {
                ToolError::execution_failed(format!("failed to run gh: {e}"))
            }
        })?;
    if !out.status.success() {
        return Err(ToolError::execution_failed(format!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn run_gh_json(context: &ToolContext, args: &[&str]) -> Result<Value, ToolError> {
    let text = run_gh_text(context, args)?;
    serde_json::from_str(&text).map_err(|e| ToolError::execution_failed(e.to_string()))
}

fn ensure_github_repo(context: &ToolContext) -> Result<(), ToolError> {
    let out = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&context.workspace)
        .output()
        .map_err(|e| ToolError::execution_failed(format!("failed to run git: {e}")))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ToolError::not_available(
            "current workspace is not a git repository",
        ))
    }
}

fn git_status_porcelain(context: &ToolContext) -> Result<String, ToolError> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&context.workspace)
        .output()
        .map_err(|e| ToolError::execution_failed(format!("failed to run git status: {e}")))?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn shape_large_text(
    context: &ToolContext,
    mut value: Value,
    label: &str,
    threshold: usize,
) -> Result<Value, ToolError> {
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    if let Some(body) = body
        && body.len() > threshold
    {
        let artifact = write_artifact_if_needed(context, label, &body, threshold)?;
        value["body_summary"] = json!(summarize(&body, 900));
        value["body_artifact"] = json!(artifact);
        value["body"] = json!(summarize(&body, 1200));
    }
    Ok(value)
}

fn write_artifact_if_needed(
    context: &ToolContext,
    label: &str,
    content: &str,
    threshold: usize,
) -> Result<Option<PathBuf>, ToolError> {
    if content.len() <= threshold {
        return Ok(None);
    }
    let Some(task_id) = context.runtime.active_task_id.as_deref() else {
        return Ok(None);
    };
    if let Some(manager) = context.runtime.task_manager.as_ref() {
        return manager
            .write_task_artifact(task_id, label, content)
            .map(Some)
            .map_err(|e| ToolError::execution_failed(e.to_string()));
    }
    let Some(data_dir) = context.runtime.task_data_dir.as_ref() else {
        return Ok(None);
    };
    let dir = data_dir.join("artifacts").join(task_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ToolError::execution_failed(format!("create artifact dir: {e}")))?;
    let absolute = dir.join(format!(
        "{}_{}.txt",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        sanitize_filename(label)
    ));
    std::fs::write(&absolute, content)
        .map_err(|e| ToolError::execution_failed(format!("write artifact: {e}")))?;
    Ok(Some(
        absolute
            .strip_prefix(data_dir)
            .map(Path::to_path_buf)
            .unwrap_or(absolute),
    ))
}

fn artifact_refs_from_context(content: &str, label: &str) -> Vec<TaskArtifactRef> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Vec::new();
    };
    let (path_key, summary_key) = if label.ends_with("_diff") {
        ("diff_artifact", "diff_summary")
    } else {
        ("body_artifact", "body_summary")
    };
    let mut refs = Vec::new();
    collect_artifact_refs(&value, path_key, summary_key, label, &mut refs);
    refs
}

fn collect_artifact_refs(
    value: &Value,
    path_key: &str,
    summary_key: &str,
    label: &str,
    refs: &mut Vec<TaskArtifactRef>,
) {
    match value {
        Value::Object(map) => {
            if let Some(path) = map.get(path_key).and_then(Value::as_str) {
                let summary = map
                    .get(summary_key)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| format!("GitHub {label} artifact"));
                refs.push(TaskArtifactRef {
                    label: label.to_string(),
                    path: PathBuf::from(path),
                    summary,
                    created_at: Utc::now(),
                });
            }
            for child in map.values() {
                collect_artifact_refs(child, path_key, summary_key, label, refs);
            }
        }
        Value::Array(items) => {
            for child in items {
                collect_artifact_refs(child, path_key, summary_key, label, refs);
            }
        }
        _ => {}
    }
}

fn github_event_metadata(
    action: &str,
    target: &str,
    number: u64,
    summary: String,
    url: Option<String>,
    artifact: Option<PathBuf>,
) -> Value {
    let artifacts = artifact
        .map(|path| {
            json!([TaskArtifactRef {
                label: format!("github_{action}"),
                path,
                summary: summary.clone(),
                created_at: Utc::now(),
            }])
        })
        .unwrap_or_else(|| json!([]));
    json!({
        "task_updates": {
            "github_event": TaskGithubEvent {
                id: format!("gh_{}", &Uuid::new_v4().to_string()[..8]),
                action: action.to_string(),
                target: target.to_string(),
                number,
                summary,
                url,
                recorded_at: Utc::now(),
            },
            "artifacts": artifacts
        }
    })
}

fn validate_evidence(input: &Value, closing: bool) -> Result<(), ToolError> {
    let evidence = input
        .get("evidence")
        .and_then(Value::as_object)
        .ok_or_else(|| ToolError::invalid_input("evidence object is required"))?;
    if closing {
        let criteria = input
            .get("acceptance_criteria")
            .and_then(Value::as_array)
            .filter(|items| !items.is_empty())
            .ok_or_else(|| ToolError::invalid_input("acceptance_criteria must be non-empty"))?;
        if criteria
            .iter()
            .any(|item| item.as_str().unwrap_or("").trim().is_empty())
        {
            return Err(ToolError::invalid_input(
                "acceptance_criteria entries must be non-empty",
            ));
        }
        for key in ["files_changed", "tests_run", "final_status"] {
            if !evidence.contains_key(key) {
                return Err(ToolError::invalid_input(format!(
                    "closure evidence missing {key}"
                )));
            }
        }
    }
    Ok(())
}

fn summarize(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (idx, ch) in text.chars().enumerate() {
        if idx >= limit.saturating_sub(3) {
            out.push_str("...");
            return out;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
    }
    out
}

fn sanitize_filename(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "artifact".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::spec::ToolSpec;

    #[test]
    fn close_schema_requires_structured_evidence() {
        let schema = GithubCloseIssueTool.input_schema();
        assert!(
            schema["properties"]["evidence"]["required"]
                .as_array()
                .expect("required")
                .contains(&json!("tests_run"))
        );
    }

    #[test]
    fn missing_close_evidence_refuses() {
        let input = json!({
            "number": 1,
            "acceptance_criteria": ["done"],
            "evidence": { "files_changed": [] }
        });
        let err = validate_evidence(&input, true).expect_err("should refuse");
        assert!(err.to_string().contains("tests_run"));
    }
}
