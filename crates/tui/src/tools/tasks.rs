//! Durable task, gate, and PR-attempt tools.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::process::Command;
use uuid::Uuid;

use crate::command_safety::{SafetyLevel, analyze_command};
use crate::dependencies::ExternalTool;
use crate::task_manager::{
    NewTaskRequest, TaskArtifactRef, TaskAttemptRecord, TaskGateRecord, TaskRecord,
};
use crate::tools::shell::{ExecShellTool, ShellWaitTool};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64, required_str,
};

const MAX_SUMMARY_CHARS: usize = 900;
const DEFAULT_GATE_TIMEOUT_MS: u64 = 120_000;
const MAX_GATE_TIMEOUT_MS: u64 = 600_000;

fn validate_storage_component(value: &str, field: &str) -> Result<(), ToolError> {
    let path = Path::new(value);
    let mut components = path.components();
    let Some(component) = components.next() else {
        return Err(ToolError::invalid_input(format!(
            "{field} must not be empty"
        )));
    };
    if components.next().is_some() || !matches!(component, std::path::Component::Normal(_)) {
        return Err(ToolError::invalid_input(format!(
            "{field} must be a single path component"
        )));
    }
    Ok(())
}

fn build_gate_command_parts(command: &str) -> (String, Vec<String>) {
    (
        "/bin/sh".to_string(),
        vec!["-lc".to_string(), command.to_string()],
    )
}

fn build_gate_command(command: &str, cwd: &Path) -> Command {
    let (program, args) = build_gate_command_parts(command);
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

pub struct TaskCreateTool;
pub struct TaskListTool;
pub struct TaskReadTool;
pub struct TaskCancelTool;
pub struct TaskGateRunTool;
pub struct TaskShellStartTool;
pub struct TaskShellWaitTool;
pub struct PrAttemptRecordTool;
pub struct PrAttemptListTool;
pub struct PrAttemptReadTool;
pub struct PrAttemptPreflightTool;

#[async_trait]
impl ToolSpec for TaskCreateTool {
    fn name(&self) -> &'static str {
        "task_create"
    }

    fn description(&self) -> &'static str {
        "Create/enqueue a durable background task through TaskManager. Durable tasks are restart-aware executable work, distinct from sub-agents."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Work prompt for the durable task." },
                "model": { "type": "string" },
                "workspace": { "type": "string", "description": "Workspace path; defaults to current workspace." },
                "mode": { "type": "string", "enum": ["agent", "plan", "yolo"] },
                "allow_shell": { "type": "boolean" },
                "trust_mode": { "type": "boolean" },
                "auto_approve": { "type": "boolean" }
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let workspace = optional_str(&input, "workspace")
            .map(PathBuf::from)
            .unwrap_or_else(|| context.workspace.clone());
        let req = NewTaskRequest {
            prompt: required_str(&input, "prompt")?.to_string(),
            model: optional_str(&input, "model").map(ToString::to_string),
            workspace: Some(workspace),
            mode: optional_str(&input, "mode").map(ToString::to_string),
            allow_shell: input.get("allow_shell").and_then(Value::as_bool),
            trust_mode: input.get("trust_mode").and_then(Value::as_bool),
            auto_approve: input.get("auto_approve").and_then(Value::as_bool),
        };
        let task = manager
            .add_task(req)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        task_result("task_create", &task)
    }
}

#[async_trait]
impl ToolSpec for TaskListTool {
    fn name(&self) -> &'static str {
        "task_list"
    }

    fn description(&self) -> &'static str {
        "List recent durable tasks with status, linked thread/turn ids, and concise summaries."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
            },
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let limit = optional_u64(&input, "limit", 20).clamp(1, 100) as usize;
        let tasks = manager.list_tasks(Some(limit)).await;
        ToolResult::json(&json!({
            "summary": format!("{} durable task(s)", tasks.len()),
            "tasks": tasks,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for TaskReadTool {
    fn name(&self) -> &'static str {
        "task_read"
    }

    fn description(&self) -> &'static str {
        "Read durable task detail including timeline, checklist, gate evidence, artifacts, and PR attempts."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Full task id or unambiguous prefix." }
            },
            "required": ["task_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let task = manager
            .get_task(required_str(&input, "task_id")?)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        task_result("task_read", &task)
    }
}

#[async_trait]
impl ToolSpec for TaskCancelTool {
    fn name(&self) -> &'static str {
        "task_cancel"
    }

    fn description(&self) -> &'static str {
        "Cancel a queued or running durable task through TaskManager. Requires approval because it changes work state."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Full task id or unambiguous prefix." }
            },
            "required": ["task_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::RequiresApproval]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let task = manager
            .cancel_task(required_str(&input, "task_id")?)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        task_result("task_cancel", &task)
    }
}

#[async_trait]
impl ToolSpec for TaskGateRunTool {
    fn name(&self) -> &'static str {
        "task_gate_run"
    }

    fn description(&self) -> &'static str {
        "Run an approved verification gate command and return structured evidence. When inside a durable task, the gate result and log artifact are attached to that task."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "gate": {
                    "type": "string",
                    "enum": ["fmt", "check", "clippy", "test", "custom"],
                    "description": "Gate category."
                },
                "command": { "type": "string", "description": "Command to run." },
                "cwd": { "type": "string", "description": "Optional working directory within the workspace." },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 600000 }
            },
            "required": ["gate", "command"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let gate = required_str(&input, "gate")?.to_string();
        let command = required_str(&input, "command")?.to_string();
        let timeout_ms = optional_u64(&input, "timeout_ms", DEFAULT_GATE_TIMEOUT_MS)
            .clamp(1_000, MAX_GATE_TIMEOUT_MS);
        let cwd = resolve_cwd(context, optional_str(&input, "cwd"))?;

        let safety = analyze_command(&command);
        if !context.auto_approve && matches!(safety.level, SafetyLevel::Dangerous) {
            return Ok(ToolResult::error(format!(
                "BLOCKED: gate command classified dangerous: {}",
                safety.reasons.join("; ")
            ))
            .with_metadata(json!({
                "safety_level": "dangerous",
                "blocked": true,
                "reasons": safety.reasons,
            })));
        }

        let started = Instant::now();
        let mut cmd = build_gate_command(&command, &cwd);
        let output =
            tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), cmd.output()).await;

        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let (exit_code, stdout, stderr, timed_out, spawn_error) = match output {
            Ok(Ok(out)) => (
                out.status.code(),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
                false,
                None,
            ),
            Ok(Err(err)) => (
                None,
                String::new(),
                String::new(),
                false,
                Some(err.to_string()),
            ),
            Err(_) => (None, String::new(), String::new(), true, None),
        };

        let full_log = format!(
            "$ {command}\n\n[stdout]\n{stdout}\n\n[stderr]\n{stderr}\n{}",
            spawn_error
                .as_ref()
                .map(|e| format!("\n[spawn_error]\n{e}\n"))
                .unwrap_or_default()
        );
        let summary_source = if !stderr.trim().is_empty() {
            stderr.as_str()
        } else if !stdout.trim().is_empty() {
            stdout.as_str()
        } else {
            spawn_error.as_deref().unwrap_or("(no output)")
        };
        let summary = summarize(summary_source, MAX_SUMMARY_CHARS);
        let status = if timed_out {
            "timeout"
        } else if spawn_error.is_some() {
            "failed"
        } else if exit_code == Some(0) {
            "passed"
        } else {
            "failed"
        };
        let classification = classify_gate_failure(&gate, status, timed_out, &stderr, &stdout);
        let log_path = write_runtime_artifact(context, "gate", &full_log)?;
        let gate_record = TaskGateRecord {
            id: format!("gate_{}", &Uuid::new_v4().to_string()[..8]),
            gate: gate.clone(),
            command: command.clone(),
            cwd: cwd.clone(),
            exit_code,
            status: status.to_string(),
            classification,
            duration_ms,
            summary: summary.clone(),
            log_path: log_path.clone(),
            recorded_at: Utc::now(),
        };

        let content = json!({
            "gate": gate_record,
            "stdout_summary": summarize(&stdout, MAX_SUMMARY_CHARS),
            "stderr_summary": summarize(&stderr, MAX_SUMMARY_CHARS),
        });
        let mut metadata = json!({
            "command": command,
            "cwd": cwd,
            "exit_code": exit_code,
            "duration_ms": duration_ms,
            "timed_out": timed_out,
            "task_updates": {
                "gate": gate_record,
                "artifacts": artifact_updates("gate_log", log_path.clone(), &summary)
            }
        });
        if let Some(path) = log_path {
            metadata["artifact_path"] = json!(path);
        }
        Ok(ToolResult::json(&content)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?
            .with_metadata(metadata))
    }
}

#[async_trait]
impl ToolSpec for TaskShellStartTool {
    fn name(&self) -> &'static str {
        "task_shell_start"
    }

    fn description(&self) -> &'static str {
        "Start a long-running shell command in the background and return a shell task_id immediately. Completion is tracked in the task/status surface; use task_shell_wait for early output, explicit barriers, or gate evidence on the active durable task."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "cwd": { "type": "string", "description": "Optional working directory within the workspace." },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 600000 },
                "stdin": { "type": "string" },
                "tty": { "type": "boolean" }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    fn starts_detached_for(&self, input: &Value) -> bool {
        input.get("command").and_then(Value::as_str).is_some()
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let mut shell_input = json!({
            "command": required_str(&input, "command")?,
            "background": true,
            "timeout_ms": optional_u64(&input, "timeout_ms", DEFAULT_GATE_TIMEOUT_MS)
                .clamp(1_000, MAX_GATE_TIMEOUT_MS),
        });
        if let Some(cwd) = optional_str(&input, "cwd") {
            let cwd = resolve_cwd(context, Some(cwd))?;
            shell_input["cwd"] = json!(cwd);
        }
        if let Some(stdin) = optional_str(&input, "stdin") {
            shell_input["stdin"] = json!(stdin);
        }
        if optional_bool(&input, "tty", false) {
            shell_input["tty"] = json!(true);
        }
        let mut result = ExecShellTool.execute(shell_input, context).await?;
        if let Some(metadata) = result.metadata.as_mut() {
            metadata["background"] = json!(true);
            metadata["task_shell"] = json!(true);
        }
        Ok(result)
    }
}

#[async_trait]
impl ToolSpec for TaskShellWaitTool {
    fn name(&self) -> &'static str {
        "task_shell_wait"
    }

    fn description(&self) -> &'static str {
        "Poll a background shell task without blocking the agent indefinitely. If `gate` is supplied and the shell task has completed, records structured gate evidence on the active durable task."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Background shell task id returned by task_shell_start or exec_shell." },
                "wait": { "type": "boolean", "default": false },
                "timeout_ms": { "type": "integer", "minimum": 1000, "maximum": 600000 },
                "gate": { "type": "string", "enum": ["fmt", "check", "clippy", "test", "custom"] },
                "command": { "type": "string", "description": "Original command, used when recording gate evidence." }
            },
            "required": ["task_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let result = ShellWaitTool::new("exec_shell_wait")
            .execute(input.clone(), context)
            .await?;
        let Some(gate) = optional_str(&input, "gate") else {
            return Ok(result);
        };
        let status = result
            .metadata
            .as_ref()
            .and_then(|m| m.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("Running");
        if status == "Running" {
            return Ok(result);
        }
        let exit_code = result
            .metadata
            .as_ref()
            .and_then(|m| m.get("exit_code"))
            .and_then(Value::as_i64)
            .and_then(|v| i32::try_from(v).ok());
        let duration_ms = result
            .metadata
            .as_ref()
            .and_then(|m| m.get("duration_ms"))
            .and_then(Value::as_u64)
            .unwrap_or_default();
        let command = optional_str(&input, "command").unwrap_or("(background shell)");
        let log_path = write_runtime_artifact(context, "background_gate", &result.content)?;
        let gate_status = if exit_code == Some(0) {
            "passed"
        } else if status == "TimedOut" {
            "timeout"
        } else {
            "failed"
        };
        let gate_record = TaskGateRecord {
            id: format!("gate_{}", &Uuid::new_v4().to_string()[..8]),
            gate: gate.to_string(),
            command: command.to_string(),
            cwd: context.workspace.clone(),
            exit_code,
            status: gate_status.to_string(),
            classification: classify_gate_failure(
                gate,
                gate_status,
                status == "TimedOut",
                &result.content,
                "",
            ),
            duration_ms,
            summary: summarize(&result.content, MAX_SUMMARY_CHARS),
            log_path: log_path.clone(),
            recorded_at: Utc::now(),
        };
        let mut metadata = result.metadata.clone().unwrap_or_else(|| json!({}));
        metadata["background"] = json!(true);
        metadata["task_updates"] = json!({
            "gate": gate_record,
            "artifacts": artifact_updates("background_gate_log", log_path, "Background shell gate output")
        });
        Ok(result.with_metadata(metadata))
    }
}

#[async_trait]
impl ToolSpec for PrAttemptRecordTool {
    fn name(&self) -> &'static str {
        "pr_attempt_record"
    }

    fn description(&self) -> &'static str {
        "Capture current git diff as a durable PR work attempt with patch artifact, changed files, and verification notes."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Task to attach to; defaults to active task." },
                "attempt_group_id": { "type": "string" },
                "attempt_index": { "type": "integer", "minimum": 1 },
                "attempt_count": { "type": "integer", "minimum": 1 },
                "summary": { "type": "string" },
                "verification": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["summary"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Auto
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let task_id = task_id_from_input_or_context(&input, context)?;
        let base_sha = git_output(&context.workspace, &["rev-parse", "HEAD"]).ok();
        let head_sha = base_sha.clone();
        let branch = git_output(&context.workspace, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
        let diff = git_output(&context.workspace, &["diff", "--binary", "--no-color"])?;
        if diff.trim().is_empty() {
            return Ok(ToolResult::error(
                "No working-tree diff to record as an attempt.",
            ));
        }
        let changed_files = git_output(&context.workspace, &["diff", "--name-only"])?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let patch_path = write_task_artifact_for(context, &task_id, "attempt_patch", &diff)?;
        let attempt = TaskAttemptRecord {
            id: format!("attempt_{}", &Uuid::new_v4().to_string()[..8]),
            attempt_group_id: optional_str(&input, "attempt_group_id")
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("attempt_group_{}", &Uuid::new_v4().to_string()[..8])),
            attempt_index: optional_u64(&input, "attempt_index", 1).max(1) as u32,
            attempt_count: optional_u64(&input, "attempt_count", 1).max(1) as u32,
            base_ref: branch.clone(),
            base_sha,
            head_ref: branch,
            head_sha,
            summary: required_str(&input, "summary")?.to_string(),
            changed_files,
            patch_path: patch_path.clone(),
            verification: input
                .get("verification")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToString::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            selected: false,
            recorded_at: Utc::now(),
        };
        let metadata = json!({
            "task_id": task_id,
            "task_updates": {
                "attempt": attempt,
                "artifacts": artifact_updates("attempt_patch", patch_path.clone(), "Captured git diff for PR attempt")
            }
        });
        if context.runtime.active_task_id.as_deref() != Some(task_id.as_str())
            && let Some(manager) = context.runtime.task_manager.as_ref()
        {
            manager
                .record_tool_metadata(&task_id, &metadata)
                .await
                .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        }
        Ok(ToolResult::json(&metadata)
            .map_err(|e| ToolError::execution_failed(e.to_string()))?
            .with_metadata(metadata))
    }
}

#[async_trait]
impl ToolSpec for PrAttemptListTool {
    fn name(&self) -> &'static str {
        "pr_attempt_list"
    }

    fn description(&self) -> &'static str {
        "List PR attempts recorded on a durable task."
    }

    fn input_schema(&self) -> Value {
        task_id_schema()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let task = read_task_for_input(&input, context).await?;
        ToolResult::json(&json!({ "task_id": task.id, "attempts": task.attempts }))
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for PrAttemptReadTool {
    fn name(&self) -> &'static str {
        "pr_attempt_read"
    }

    fn description(&self) -> &'static str {
        "Read one recorded PR attempt and its patch artifact reference."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Task id; defaults to active task." },
                "attempt_id": { "type": "string" }
            },
            "required": ["attempt_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let task = read_task_for_input(&input, context).await?;
        let attempt_id = required_str(&input, "attempt_id")?;
        let attempt = task
            .attempts
            .iter()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| ToolError::invalid_input(format!("Attempt not found: {attempt_id}")))?;
        ToolResult::json(attempt).map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for PrAttemptPreflightTool {
    fn name(&self) -> &'static str {
        "pr_attempt_preflight"
    }

    fn description(&self) -> &'static str {
        "Run `git apply --check` for a recorded attempt patch. This is a no-mutation preflight; actual apply remains explicit and approval-gated elsewhere."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Task id; defaults to active task." },
                "attempt_id": { "type": "string" }
            },
            "required": ["attempt_id"],
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::ReadOnly]
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let task = read_task_for_input(&input, context).await?;
        let attempt_id = required_str(&input, "attempt_id")?;
        let attempt = task
            .attempts
            .iter()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| ToolError::invalid_input(format!("Attempt not found: {attempt_id}")))?;
        let patch_ref = attempt
            .patch_path
            .as_ref()
            .ok_or_else(|| ToolError::invalid_input("Attempt has no patch artifact"))?;
        let patch_path = manager.artifact_absolute_path(patch_ref);
        let workspace = context.workspace.clone();
        let out = tokio::task::spawn_blocking(move || {
            crate::dependencies::Git::command()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "git not found"))?
                .args(["apply", "--check"])
                .arg(&patch_path)
                .current_dir(&workspace)
                .output()
        })
        .await
        .map_err(|join_err| {
            ToolError::execution_failed(format!("git apply --check panicked: {join_err}"))
        })?
        .map_err(|e| ToolError::execution_failed(format!("git apply --check failed: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        Ok(ToolResult::json(&json!({
            "attempt_id": attempt_id,
            "patch_path": patch_ref,
            "would_apply": out.status.success(),
            "exit_code": out.status.code(),
            "stdout_summary": summarize(&stdout, MAX_SUMMARY_CHARS),
            "stderr_summary": summarize(&stderr, MAX_SUMMARY_CHARS),
            "mutated_worktree": false
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))?)
    }
}

fn task_result(label: &str, task: &TaskRecord) -> Result<ToolResult, ToolError> {
    ToolResult::json(&json!({
        "summary": format!("{label}: {} ({:?})", task.id, task.status),
        "task": task,
    }))
    .map_err(|e| ToolError::execution_failed(e.to_string()))
}

fn resolve_cwd(context: &ToolContext, raw: Option<&str>) -> Result<PathBuf, ToolError> {
    match raw {
        Some(path) => {
            let resolved = context.resolve_path(path)?;
            if resolved.is_dir() {
                Ok(resolved)
            } else {
                Err(ToolError::invalid_input(format!(
                    "cwd must be a directory: {path}"
                )))
            }
        }
        None => Ok(context.workspace.clone()),
    }
}

fn write_runtime_artifact(
    context: &ToolContext,
    label: &str,
    content: &str,
) -> Result<Option<PathBuf>, ToolError> {
    let Some(task_id) = context.runtime.active_task_id.as_deref() else {
        return Ok(None);
    };
    let manager = context.runtime.task_manager.as_ref();
    if let Some(manager) = manager {
        return manager
            .write_task_artifact(task_id, label, content)
            .map(Some)
            .map_err(|e| ToolError::execution_failed(e.to_string()));
    }
    let Some(data_dir) = context.runtime.task_data_dir.as_ref() else {
        return Ok(None);
    };
    validate_storage_component(task_id, "task_id")?;
    let artifact_dir = data_dir.join("artifacts").join(task_id);
    std::fs::create_dir_all(&artifact_dir)
        .map_err(|e| ToolError::execution_failed(format!("create artifact dir: {e}")))?;
    let filename = format!(
        "{}_{}.txt",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        sanitize_filename(label)
    );
    let absolute = artifact_dir.join(filename);
    std::fs::write(&absolute, content)
        .map_err(|e| ToolError::execution_failed(format!("write artifact: {e}")))?;
    Ok(Some(
        absolute
            .strip_prefix(data_dir)
            .map(PathBuf::from)
            .unwrap_or(absolute),
    ))
}

fn write_task_artifact_for(
    context: &ToolContext,
    task_id: &str,
    label: &str,
    content: &str,
) -> Result<Option<PathBuf>, ToolError> {
    if let Some(manager) = context.runtime.task_manager.as_ref() {
        return manager
            .write_task_artifact(task_id, label, content)
            .map(Some)
            .map_err(|e| ToolError::execution_failed(e.to_string()));
    }
    if context.runtime.active_task_id.as_deref() != Some(task_id) {
        return Ok(None);
    }
    write_runtime_artifact(context, label, content)
}

fn artifact_updates(label: &str, path: Option<PathBuf>, summary: &str) -> Value {
    match path {
        Some(path) => json!([TaskArtifactRef {
            label: label.to_string(),
            path,
            summary: summarize(summary, 240),
            created_at: Utc::now(),
        }]),
        None => json!([]),
    }
}

async fn read_task_for_input(
    input: &Value,
    context: &ToolContext,
) -> Result<TaskRecord, ToolError> {
    let manager = context
        .runtime
        .task_manager
        .as_ref()
        .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
    let task_id = task_id_from_input_or_context(input, context)?;
    manager
        .get_task(&task_id)
        .await
        .map_err(|e| ToolError::execution_failed(e.to_string()))
}

fn task_id_from_input_or_context(
    input: &Value,
    context: &ToolContext,
) -> Result<String, ToolError> {
    optional_str(input, "task_id")
        .map(ToString::to_string)
        .or_else(|| context.runtime.active_task_id.clone())
        .ok_or_else(|| {
            ToolError::invalid_input("task_id is required when no durable task is active")
        })
}

fn task_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "task_id": { "type": "string", "description": "Task id; defaults to active task." }
        },
        "additionalProperties": false
    })
}

fn git_output(workspace: &Path, args: &[&str]) -> Result<String, ToolError> {
    let out = crate::dependencies::Git::output(args, workspace)
        .map_err(|e| ToolError::execution_failed(format!("failed to run git: {e}")))?;
    if !out.status.success() {
        return Err(ToolError::execution_failed(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn classify_gate_failure(
    gate: &str,
    status: &str,
    timed_out: bool,
    stderr: &str,
    stdout: &str,
) -> String {
    if timed_out {
        return "timeout".to_string();
    }
    if status == "passed" {
        return "passed".to_string();
    }
    let haystack = format!("{stderr}\n{stdout}").to_ascii_lowercase();
    if haystack.contains("address already in use") || haystack.contains("port") {
        "environment_port_binding".to_string()
    } else if gate == "clippy" || haystack.contains("warning:") {
        "lint_failure".to_string()
    } else if gate == "test" || haystack.contains("test result: failed") {
        "test_failure".to_string()
    } else if haystack.contains("error: could not compile")
        || haystack.contains("compilation failed")
    {
        "compile_error".to_string()
    } else {
        "environment_or_tooling_failure".to_string()
    }
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
    if out.trim().is_empty() {
        "(no output)".to_string()
    } else {
        out
    }
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
    fn durable_task_schema_requires_prompt() {
        let schema = TaskCreateTool.input_schema();
        assert_eq!(schema["required"][0], "prompt");
        assert!(schema["properties"]["prompt"].is_object());
    }

    #[test]
    fn gate_classifier_detects_timeout() {
        assert_eq!(
            classify_gate_failure("test", "timeout", true, "", ""),
            "timeout"
        );
    }

    #[test]
    fn background_shell_schema_is_explicit() {
        let schema = TaskShellStartTool.input_schema();
        assert_eq!(schema["required"][0], "command");
        assert_eq!(schema["properties"]["timeout_ms"]["maximum"], 600000);

        let wait_schema = TaskShellWaitTool.input_schema();
        assert_eq!(wait_schema["required"][0], "task_id");
        assert!(wait_schema["properties"]["gate"].is_object());
    }

    #[test]
    fn gate_command_uses_login_shell_invocation() {
        let (program, args) = build_gate_command_parts("echo hello");
        assert_eq!(program, "/bin/sh");
        assert_eq!(args, vec!["-lc".to_string(), "echo hello".to_string()]);
    }
}
