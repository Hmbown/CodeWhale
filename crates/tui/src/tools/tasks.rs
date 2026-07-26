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
    NewTaskRequest, TaskArtifactRef, TaskAttemptRecord, TaskCancelDisposition, TaskGateRecord,
    TaskRecord,
};
use crate::tools::shell::BashTool;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64, required_str,
};
use crate::work_graph::{
    CancelOutcome, OperationIntent, OperationObservation, OperationOwnerSnapshot, OwnerState,
    task_owner_snapshot,
};

const MAX_SUMMARY_CHARS: usize = 900;
const DEFAULT_GATE_TIMEOUT_MS: u64 = 120_000;
const MAX_GATE_TIMEOUT_MS: u64 = 600_000;

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

/// Unified durable-task tool (piagent phase B).
///
/// The model sees one tool, `tasks`, with an `action` parameter routing to
/// the per-action logic below. Legacy `task_*` / `pr_attempt_*` names stay
/// registered as hidden compat aliases that force the action so saved
/// transcripts replay correctly — the pattern `BashTool` established in #4625.
///
/// `TaskShellStartTool` / `TaskShellWaitTool` stay separate: the registry
/// gates them behind `allow_shell` (see `with_runtime_task_shell_tools`),
/// which differs from every other action in this family.
pub struct TasksTool {
    name: &'static str,
    forced_action: Option<&'static str>,
    read_only: bool,
}

pub struct TaskShellStartTool;
pub struct TaskShellWaitTool;

/// Actions the Plan-mode read-only surface exposes.
const READ_ACTIONS: &[&str] = &["list", "read", "pr_attempt_list", "pr_attempt_read"];
const ALL_ACTIONS: &[&str] = &[
    "create",
    "list",
    "read",
    "cancel",
    "gate_run",
    "pr_attempt_record",
    "pr_attempt_list",
    "pr_attempt_read",
    "pr_attempt_preflight",
];

impl TasksTool {
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: false,
        }
    }

    /// Plan-mode variant: only the read-only actions are advertised and routed.
    pub const fn read_only(name: &'static str) -> Self {
        Self {
            name,
            forced_action: None,
            read_only: true,
        }
    }

    pub const fn alias(name: &'static str, action: &'static str) -> Self {
        Self {
            name,
            forced_action: Some(action),
            read_only: false,
        }
    }

    fn allowed_actions(&self) -> &'static [&'static str] {
        if self.read_only {
            READ_ACTIONS
        } else {
            ALL_ACTIONS
        }
    }

    fn resolve_action<'a>(&'a self, input: &'a Value) -> Result<&'a str, ToolError> {
        let action = match self.forced_action {
            Some(action) => action,
            None => input.get("action").and_then(Value::as_str).ok_or_else(|| {
                ToolError::invalid_input(format!(
                    "tasks: missing `action` (one of: {})",
                    self.allowed_actions().join(", ")
                ))
            })?,
        };
        if self.allowed_actions().contains(&action) {
            Ok(action)
        } else {
            Err(ToolError::invalid_input(format!(
                "tasks: invalid action `{action}` (one of: {})",
                self.allowed_actions().join(", ")
            )))
        }
    }

    fn action_is_read(action: &str) -> bool {
        READ_ACTIONS.contains(&action)
    }

    /// Whether this action executes code (drives static capabilities and the
    /// Plan-mode "no ExecutesCode tools" invariant).
    fn action_executes_code(action: &str) -> bool {
        action == "gate_run"
    }

    fn action_requires_approval(action: &str) -> bool {
        !Self::action_is_read(action)
    }
}

#[async_trait]
impl ToolSpec for TasksTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn model_visible(&self) -> bool {
        self.forced_action.is_none()
    }

    fn description(&self) -> &'static str {
        match self.forced_action {
            Some("create") => {
                "Create/enqueue a durable background task through TaskManager. Durable tasks are restart-aware executable work, distinct from sub-agents."
            }
            Some("list") => {
                "List recent durable tasks with status, linked thread/turn ids, and concise summaries."
            }
            Some("read") => {
                "Read durable task detail including timeline, checklist, gate evidence, artifacts, and PR attempts."
            }
            Some("cancel") => {
                "Cancel a queued or running durable task through TaskManager. Requires approval because it changes work state."
            }
            Some("gate_run") => {
                "Run an approved verification gate command and return structured evidence. When inside a durable task, the gate result and log artifact are attached to that task."
            }
            Some("pr_attempt_record") => {
                "Capture current git diff as a durable PR work attempt with patch artifact, changed files, and verification notes."
            }
            Some("pr_attempt_list") => "List PR attempts recorded on a durable task.",
            Some("pr_attempt_read") => {
                "Read one recorded PR attempt and its patch artifact reference."
            }
            Some("pr_attempt_preflight") => {
                "Run `git apply --check` for a recorded attempt patch. This is a no-mutation preflight; actual apply remains explicit and approval-gated elsewhere."
            }
            _ if self.read_only => {
                "Inspect durable tasks and their PR attempts. Actions: \"list\", \"read\", \"pr_attempt_list\", \"pr_attempt_read\"."
            }
            _ => {
                "Manage durable background tasks through TaskManager. Durable tasks are restart-aware executable work, distinct from sub-agents. Actions: \"create\" (enqueue; approval), \"list\", \"read\", \"cancel\" (approval), \"gate_run\" (run an approved verification gate command and return structured evidence; approval), \"pr_attempt_record\", \"pr_attempt_list\", \"pr_attempt_read\", \"pr_attempt_preflight\". Use task_shell_start for long-running shell work."
            }
        }
    }

    fn input_schema(&self) -> Value {
        if let Some(action) = self.forced_action {
            return legacy_action_schema(action);
        }
        let actions: Vec<&str> = self.allowed_actions().to_vec();
        let mut properties = serde_json::Map::new();
        properties.insert(
            "action".to_string(),
            json!({
                "type": "string",
                "enum": actions,
                "description": "Action to perform."
            }),
        );
        if !self.read_only {
            properties.insert(
                "prompt".to_string(),
                json!({ "type": "string", "description": "Work prompt for the durable task (action=create)." }),
            );
            properties.insert(
                "model".to_string(),
                json!({ "type": "string", "description": "(action=create)" }),
            );
            properties.insert(
                "workspace".to_string(),
                json!({ "type": "string", "description": "Workspace path; defaults to current workspace. (action=create)" }),
            );
            properties.insert(
                "mode".to_string(),
                json!({ "type": "string", "enum": ["agent", "plan", "yolo"], "description": "(action=create)" }),
            );
            properties.insert(
                "allow_shell".to_string(),
                json!({ "type": "boolean", "description": "(action=create)" }),
            );
            properties.insert(
                "trust_mode".to_string(),
                json!({ "type": "boolean", "description": "(action=create)" }),
            );
            properties.insert(
                "auto_approve".to_string(),
                json!({ "type": "boolean", "description": "(action=create)" }),
            );
            properties.insert(
                "gate".to_string(),
                json!({
                    "type": "string",
                    "enum": ["fmt", "check", "clippy", "test", "custom"],
                    "description": "Gate category. (action=gate_run)"
                }),
            );
            properties.insert(
                "command".to_string(),
                json!({ "type": "string", "description": "Command to run. (action=gate_run)" }),
            );
            properties.insert(
                "cwd".to_string(),
                json!({ "type": "string", "description": "Optional working directory within the workspace. (action=gate_run)" }),
            );
            properties.insert(
                "timeout_ms".to_string(),
                json!({ "type": "integer", "minimum": 1000, "maximum": 600000, "description": "(action=gate_run)" }),
            );
            properties.insert(
                "attempt_group_id".to_string(),
                json!({ "type": "string", "description": "(action=pr_attempt_record)" }),
            );
            properties.insert(
                "attempt_index".to_string(),
                json!({ "type": "integer", "minimum": 1, "description": "(action=pr_attempt_record)" }),
            );
            properties.insert(
                "attempt_count".to_string(),
                json!({ "type": "integer", "minimum": 1, "description": "(action=pr_attempt_record)" }),
            );
            properties.insert(
                "summary".to_string(),
                json!({ "type": "string", "description": "Attempt summary (action=pr_attempt_record)." }),
            );
            properties.insert(
                "verification".to_string(),
                json!({ "type": "array", "items": { "type": "string" }, "description": "(action=pr_attempt_record)" }),
            );
        }
        properties.insert(
            "attempt_id".to_string(),
            json!({ "type": "string", "description": "(action=pr_attempt_read/preflight)" }),
        );
        properties.insert(
            "task_id".to_string(),
            json!({ "type": "string", "description": "Full task id or unambiguous prefix (action=read/cancel); task id, defaults to active task (action=pr_attempt_*)." }),
        );
        properties.insert(
            "limit".to_string(),
            json!({ "type": "integer", "minimum": 1, "maximum": 100, "default": 20, "description": "(action=list)" }),
        );
        json!({
            "type": "object",
            "properties": properties,
            "additionalProperties": false
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        match self.forced_action {
            Some(action) if Self::action_executes_code(action) => {
                vec![
                    ToolCapability::ExecutesCode,
                    ToolCapability::RequiresApproval,
                ]
            }
            Some(action) if Self::action_is_read(action) => vec![ToolCapability::ReadOnly],
            Some(_) => vec![ToolCapability::RequiresApproval],
            None if self.read_only => vec![ToolCapability::ReadOnly],
            None => vec![
                ToolCapability::ExecutesCode,
                ToolCapability::RequiresApproval,
            ],
        }
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        match self.forced_action {
            Some(action) if Self::action_requires_approval(action) => ApprovalRequirement::Required,
            Some(_) => ApprovalRequirement::Auto,
            None if self.read_only => ApprovalRequirement::Auto,
            None => ApprovalRequirement::Required,
        }
    }

    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        match self.resolve_action(input) {
            Ok(action) if Self::action_requires_approval(action) => ApprovalRequirement::Required,
            Ok(_) => ApprovalRequirement::Auto,
            Err(_) => self.approval_requirement(),
        }
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        match self.resolve_action(input) {
            Ok(action) => Self::action_is_read(action),
            Err(_) => self.is_read_only(),
        }
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        match self.resolve_action(&input)? {
            "create" => self.execute_create(&input, context).await,
            "list" => self.execute_list(&input, context).await,
            "read" => self.execute_read(&input, context).await,
            "cancel" => self.execute_cancel(&input, context).await,
            "gate_run" => self.execute_gate_run(&input, context).await,
            "pr_attempt_record" => self.execute_pr_attempt_record(&input, context).await,
            "pr_attempt_list" => self.execute_pr_attempt_list(&input, context).await,
            "pr_attempt_read" => self.execute_pr_attempt_read(&input, context).await,
            "pr_attempt_preflight" => self.execute_pr_attempt_preflight(&input, context).await,
            action => Err(ToolError::invalid_input(format!(
                "tasks: invalid action `{action}`"
            ))),
        }
    }
}

/// The exact schema the legacy per-action tool exposed, kept so hidden alias
/// registrations report an identical contract to the pre-unification tools.
fn legacy_action_schema(action: &str) -> Value {
    match action {
        "create" => json!({
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
        }),
        "list" => json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 20 }
            },
            "additionalProperties": false
        }),
        "read" | "cancel" => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Full task id or unambiguous prefix." }
            },
            "required": ["task_id"],
            "additionalProperties": false
        }),
        "gate_run" => json!({
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
        }),
        "pr_attempt_record" => json!({
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
        }),
        "pr_attempt_list" => task_id_schema(),
        // pr_attempt_read / pr_attempt_preflight share the attempt-id schema.
        _ => json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Task id; defaults to active task." },
                "attempt_id": { "type": "string" }
            },
            "required": ["attempt_id"],
            "additionalProperties": false
        }),
    }
}

impl TasksTool {
    async fn execute_create(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let workspace = optional_str(input, "workspace")
            .map(PathBuf::from)
            .unwrap_or_else(|| context.workspace.clone());
        let prompt = required_str(input, "prompt")?.to_string();
        let req = NewTaskRequest {
            prompt: prompt.clone(),
            model: optional_str(input, "model").map(ToString::to_string),
            workspace: Some(workspace),
            mode: optional_str(input, "mode").map(ToString::to_string),
            allow_shell: input.get("allow_shell").and_then(Value::as_bool),
            trust_mode: input.get("trust_mode").and_then(Value::as_bool),
            auto_approve: input.get("auto_approve").and_then(Value::as_bool),
        };
        let task_id = crate::task_manager::TaskManager::new_task_id();
        if let Some(work) = context.runtime.work.as_ref() {
            work.register_operation(
                &context.state_namespace,
                OperationIntent::new(
                    format!("task:{task_id}"),
                    prompt,
                    true,
                    "task_create",
                    &task_id,
                ),
            )
            .map_err(ToolError::execution_failed)?;
        }
        let task = match manager.add_task_with_id(req, task_id.clone()).await {
            Ok(task) => task,
            Err(err) => {
                if let Some(work) = context.runtime.work.as_ref() {
                    let _ = work.reconcile_operation(
                        &context.state_namespace,
                        OperationOwnerSnapshot::new(
                            format!("task:{task_id}"),
                            OwnerState::Failed,
                            1,
                            Utc::now().timestamp_millis(),
                        ),
                    );
                }
                return Err(ToolError::execution_failed(err.to_string()));
            }
        };
        let lifecycle_warning = reconcile_task_record(context, &task).err().map(|err| {
            tracing::warn!(task_id = %task.id, error = %err, "task was created but Work lifecycle reconciliation failed");
            err.to_string()
        });
        task_result_with_lifecycle_warning("task_create", &task, lifecycle_warning.as_deref())
    }

    async fn execute_list(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let limit = optional_u64(input, "limit", 20).clamp(1, 100) as usize;
        let tasks = manager.list_tasks(Some(limit)).await;
        ToolResult::json(&json!({
            "summary": format!("{} durable task(s)", tasks.len()),
            "tasks": tasks,
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_read(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let task = manager
            .get_task(required_str(input, "task_id")?)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        task_result("task_read", &task)
    }

    async fn execute_cancel(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let cancellation = manager
            .cancel_task(required_str(input, "task_id")?)
            .await
            .map_err(|e| ToolError::execution_failed(e.to_string()))?;
        let task = cancellation.task;
        let cancel_outcome = match cancellation.disposition {
            TaskCancelDisposition::Forced => CancelOutcome::Forced,
            TaskCancelDisposition::Requested => CancelOutcome::Requested,
            TaskCancelDisposition::AlreadyFinished => CancelOutcome::AlreadyFinished,
        };
        let mut lifecycle_warnings = Vec::new();
        if let Some(work) = context.runtime.work.as_ref() {
            let external = format!("task:{}", task.id);
            if work.has_operation_binding(Some(&context.state_namespace), &external)
                && let Err(err) = work.reconcile_observation(
                    &context.state_namespace,
                    &external,
                    OperationObservation::CancelUpdate {
                        outcome: cancel_outcome,
                        at: Utc::now().timestamp_millis(),
                    },
                )
            {
                tracing::warn!(task_id = %task.id, error = %err, "task was cancelled but Work cancel reconciliation failed");
                lifecycle_warnings.push(err);
            }
        }
        if let Err(err) = reconcile_task_record(context, &task) {
            tracing::warn!(task_id = %task.id, error = %err, "task cancellation succeeded but owner-state reconciliation failed");
            lifecycle_warnings.push(err.to_string());
        }
        let lifecycle_warning =
            (!lifecycle_warnings.is_empty()).then(|| lifecycle_warnings.join("; "));
        task_result_with_lifecycle_warning("task_cancel", &task, lifecycle_warning.as_deref())
    }

    async fn execute_gate_run(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let gate = required_str(input, "gate")?.to_string();
        let command = required_str(input, "command")?.to_string();
        let timeout_ms = optional_u64(input, "timeout_ms", DEFAULT_GATE_TIMEOUT_MS)
            .clamp(1_000, MAX_GATE_TIMEOUT_MS);
        let cwd = resolve_cwd(context, optional_str(input, "cwd"))?;

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
        let log_path = write_runtime_artifact(context, "gate", &full_log).await?;
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

    async fn execute_pr_attempt_record(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task_id = task_id_from_input_or_context(input, context)?;
        let base_sha = git_output(&context.workspace, &["rev-parse", "HEAD"])
            .await
            .ok();
        let head_sha = base_sha.clone();
        let branch = git_output(&context.workspace, &["rev-parse", "--abbrev-ref", "HEAD"])
            .await
            .ok();
        let diff = git_output(&context.workspace, &["diff", "--binary", "--no-color"]).await?;
        if diff.trim().is_empty() {
            return Ok(ToolResult::error(
                "No working-tree diff to record as an attempt.",
            ));
        }
        let changed_files = git_output(&context.workspace, &["diff", "--name-only"])
            .await?
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let patch_path = write_task_artifact_for(context, &task_id, "attempt_patch", &diff).await?;
        let attempt = TaskAttemptRecord {
            id: format!("attempt_{}", &Uuid::new_v4().to_string()[..8]),
            attempt_group_id: optional_str(input, "attempt_group_id")
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("attempt_group_{}", &Uuid::new_v4().to_string()[..8])),
            attempt_index: optional_u64(input, "attempt_index", 1).max(1) as u32,
            attempt_count: optional_u64(input, "attempt_count", 1).max(1) as u32,
            base_ref: branch.clone(),
            base_sha,
            head_ref: branch,
            head_sha,
            summary: required_str(input, "summary")?.to_string(),
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

    async fn execute_pr_attempt_list(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task = read_task_for_input(input, context).await?;
        ToolResult::json(&json!({ "task_id": task.id, "attempts": task.attempts }))
            .map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_pr_attempt_read(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let task = read_task_for_input(input, context).await?;
        let attempt_id = required_str(input, "attempt_id")?;
        let attempt = task
            .attempts
            .iter()
            .find(|attempt| attempt.id == attempt_id)
            .ok_or_else(|| ToolError::invalid_input(format!("Attempt not found: {attempt_id}")))?;
        ToolResult::json(attempt).map_err(|e| ToolError::execution_failed(e.to_string()))
    }

    async fn execute_pr_attempt_preflight(
        &self,
        input: &Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let manager = context
            .runtime
            .task_manager
            .as_ref()
            .ok_or_else(|| ToolError::not_available("TaskManager is not attached"))?;
        let task = read_task_for_input(input, context).await?;
        let attempt_id = required_str(input, "attempt_id")?;
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
            // Surface the otherwise-discarded join error for debugging; the
            // returned ToolError (and thus user-facing behavior) is unchanged.
            tracing::debug!(error = %join_err, "git apply --check spawn_blocking task failed to join");
            ToolError::execution_failed(format!("git apply --check panicked: {join_err}"))
        })?
        .map_err(|e| ToolError::execution_failed(format!("git apply --check failed: {e}")))?;
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        ToolResult::json(&json!({
            "attempt_id": attempt_id,
            "patch_path": patch_ref,
            "would_apply": out.status.success(),
            "exit_code": out.status.code(),
            "stdout_summary": summarize(&stdout, MAX_SUMMARY_CHARS),
            "stderr_summary": summarize(&stderr, MAX_SUMMARY_CHARS),
            "mutated_worktree": false
        }))
        .map_err(|e| ToolError::execution_failed(e.to_string()))
    }
}

#[async_trait]
impl ToolSpec for TaskShellStartTool {
    fn name(&self) -> &'static str {
        "task_shell_start"
    }

    fn description(&self) -> &'static str {
        "Start a long-running shell command in the background and return a shell task_id immediately. Completion is delivered automatically as an internal runtime event and remains visible in the task/status surface; use task_shell_wait only for early output, explicit barriers, or gate evidence on the active durable task."
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
        let mut result = BashTool::new("Bash").execute(shell_input, context).await?;
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
        "Poll a background shell task without blocking the agent indefinitely. Completion is delivered automatically; use this only for early output, explicit barriers, or gate evidence. If `gate` is supplied and the shell task has completed, records structured gate evidence on the active durable task."
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
        let result = BashTool::alias("exec_shell_wait", "wait")
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
        let log_path = write_runtime_artifact(context, "background_gate", &result.content).await?;
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

fn reconcile_task_record(context: &ToolContext, task: &TaskRecord) -> Result<(), ToolError> {
    let Some(work) = context.runtime.work.as_ref() else {
        return Ok(());
    };
    let external = format!("task:{}", task.id);
    if !work.has_operation_binding(Some(&context.state_namespace), &external) {
        return Ok(());
    }
    work.reconcile_operation(
        &context.state_namespace,
        task_owner_snapshot(
            &task.id,
            task.status,
            task.lifecycle_seq,
            task.created_at,
            task.started_at,
            task.ended_at,
        ),
    )
    .map(|_| ())
    .map_err(ToolError::execution_failed)
}

fn task_result(label: &str, task: &TaskRecord) -> Result<ToolResult, ToolError> {
    task_result_with_lifecycle_warning(label, task, None)
}

fn task_result_with_lifecycle_warning(
    label: &str,
    task: &TaskRecord,
    lifecycle_warning: Option<&str>,
) -> Result<ToolResult, ToolError> {
    ToolResult::json(&json!({
        "summary": format!("{label}: {} ({:?})", task.id, task.status),
        "task": task,
        "lifecycle_warning": lifecycle_warning,
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

async fn write_runtime_artifact(
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
    let artifact_dir = data_dir.join("artifacts").join(task_id);
    let filename = format!(
        "{}_{}.txt",
        Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
        sanitize_filename(label)
    );
    let absolute = artifact_dir.join(filename);
    let content_owned = content.to_owned();
    let abs = absolute.clone();
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&artifact_dir)?;
        std::fs::write(&abs, content_owned)?;
        Ok::<(), std::io::Error>(())
    })
    .await
    .map_err(|e| {
        // Surface the otherwise-discarded join error for debugging; the
        // returned ToolError (and thus user-facing behavior) is unchanged.
        tracing::debug!(error = %e, "artifact write spawn_blocking task failed to join");
        ToolError::execution_failed(format!("artifact write task panicked: {e}"))
    })?
    .map_err(|e| ToolError::execution_failed(format!("write artifact: {e}")))?;
    Ok(Some(
        absolute
            .strip_prefix(data_dir)
            .map(PathBuf::from)
            .unwrap_or(absolute),
    ))
}

async fn write_task_artifact_for(
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
    write_runtime_artifact(context, label, content).await
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

async fn git_output(workspace: &Path, args: &[&str]) -> Result<String, ToolError> {
    let args_owned: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    let cwd = workspace.to_path_buf();
    let out = tokio::task::spawn_blocking(move || {
        let arg_refs: Vec<&str> = args_owned.iter().map(String::as_str).collect();
        crate::dependencies::Git::output(&arg_refs, &cwd)
    })
    .await
    .map_err(|e| {
        // Surface the otherwise-discarded join error for debugging; the
        // returned ToolError (and thus user-facing behavior) is unchanged.
        tracing::debug!(error = %e, "git spawn_blocking task failed to join");
        ToolError::execution_failed(format!("git task panicked: {e}"))
    })?
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
        let schema = TasksTool::alias("task_create", "create").input_schema();
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
    fn canonical_schema_lists_all_actions_and_union_fields() {
        let schema = TasksTool::new("tasks").input_schema();
        let actions = schema["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        for action in [
            "create",
            "list",
            "read",
            "cancel",
            "gate_run",
            "pr_attempt_record",
            "pr_attempt_list",
            "pr_attempt_read",
            "pr_attempt_preflight",
        ] {
            assert!(
                actions.iter().any(|value| value.as_str() == Some(action)),
                "canonical schema must offer action {action}"
            );
        }
        for field in [
            "prompt",
            "task_id",
            "gate",
            "command",
            "attempt_id",
            "limit",
        ] {
            assert!(
                schema["properties"][field].is_object(),
                "canonical schema must carry union field {field}"
            );
        }
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn read_only_variant_only_offers_read_actions() {
        let tool = TasksTool::read_only("tasks");
        let schema = tool.input_schema();
        assert_eq!(
            schema["properties"]["action"]["enum"],
            json!(["list", "read", "pr_attempt_list", "pr_attempt_read"])
        );
        assert!(!schema["properties"]["prompt"].is_object());
        assert!(!schema["properties"]["gate"].is_object());
        // pr_attempt_read is a read action: its id field must be advertised
        // on the read-only surface too.
        assert!(schema["properties"]["attempt_id"].is_object());
        assert!(schema["properties"]["task_id"].is_object());
        assert_eq!(tool.approval_requirement(), ApprovalRequirement::Auto);
        assert!(tool.is_read_only());
        assert_eq!(tool.capabilities(), vec![ToolCapability::ReadOnly]);
    }

    #[test]
    fn aliases_hide_from_model_and_force_action() {
        let create = TasksTool::alias("task_create", "create");
        assert!(!create.model_visible());
        assert_eq!(create.name(), "task_create");
        assert_eq!(create.approval_requirement(), ApprovalRequirement::Required);

        let gate = TasksTool::alias("task_gate_run", "gate_run");
        assert_eq!(gate.approval_requirement(), ApprovalRequirement::Required);
        assert!(gate.capabilities().contains(&ToolCapability::ExecutesCode));

        let list = TasksTool::alias("task_list", "list");
        assert_eq!(list.approval_requirement(), ApprovalRequirement::Auto);
        assert!(list.is_read_only_for(&json!({})));

        let canonical = TasksTool::new("tasks");
        assert!(canonical.model_visible());
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "list"})),
            ApprovalRequirement::Auto
        );
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "cancel"})),
            ApprovalRequirement::Required
        );
        assert_eq!(
            canonical.approval_requirement_for(&json!({"action": "gate_run"})),
            ApprovalRequirement::Required
        );
        assert!(canonical.is_read_only_for(&json!({"action": "pr_attempt_read"})));
        assert!(!canonical.is_read_only_for(&json!({"action": "create"})));
    }

    #[test]
    fn canonical_rejects_unknown_or_missing_action() {
        let tool = TasksTool::new("tasks");
        let err = tool
            .resolve_action(&json!({}))
            .expect_err("missing action must fail");
        assert!(err.to_string().contains("missing `action`"));
        let err = tool
            .resolve_action(&json!({"action": "explode"}))
            .expect_err("unknown action must fail");
        assert!(err.to_string().contains("invalid action"));

        let read_only = TasksTool::read_only("tasks");
        let err = read_only
            .resolve_action(&json!({"action": "gate_run"}))
            .expect_err("read-only surface must reject exec actions");
        assert!(err.to_string().contains("invalid action"));
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
