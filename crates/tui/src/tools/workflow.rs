//! Model-facing Workflow runner over the live sub-agent runtime.
//!
//! The JS VM stays in `codewhale-workflow-js`; this module supplies the TUI
//! driver that turns each `task(...)` call into a real `SubAgentManager` spawn.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use codewhale_workflow::{
    AgentType, BranchResult, BranchSpec, BudgetSpec, ControlNodeKind, ControlNodeResult,
    LeafResult, LeafSpec, ReduceSpec, SequenceSpec, TaskMode,
    WorkflowExecution as IrWorkflowExecution, WorkflowMemoUsage, WorkflowNode,
    WorkflowRunStatus as IrWorkflowRunStatus, WorkflowSpec, WorkflowUsage,
    compile_javascript_workflow, compile_typescript_workflow, leaf_wants_worktree,
};
use codewhale_workflow_js::{
    BudgetSnapshot, DriverError, ProgressEvent, SpawnedTask, TaskCompletion, TaskRequest,
    WORKFLOW_MAX_CONCURRENT, WorkflowDriver, WorkflowRunCancel, WorkflowVm,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot};
use uuid::Uuid;

use crate::core::events::Event;
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
    optional_bool, optional_str, optional_u64,
};
use crate::tools::subagent::{
    SharedSubAgentManager, SubAgentCompletion, SubAgentRuntime, SubAgentStatus,
    WorkflowTaskSpawnIdentity, WorkflowTaskSpawnMetadata, spawn_workflow_task,
};
use crate::tools::verifier::run_workflow_completion_gates;
use crate::tools::workflow_plan_approval::{
    WorkflowPlanApprovalReceipt, analyze_workflow_plan_approval_with_config, analyze_workflow_spec,
    workflow_approval_requirement_for,
};
use crate::utils::spawn_supervised;

#[derive(Clone)]
pub struct WorkflowTool {
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
}

impl WorkflowTool {
    #[must_use]
    pub fn new(manager: SharedSubAgentManager, runtime: SubAgentRuntime) -> Self {
        Self { manager, runtime }
    }
}

type SharedWorkflowRuns = Arc<Mutex<HashMap<String, WorkflowRunRecord>>>;
type SharedWorkflowControllers = Arc<Mutex<HashMap<String, Arc<WorkflowRunController>>>>;

struct WorkflowRunController {
    driver: Arc<SubAgentWorkflowDriver>,
    vm_cancel: WorkflowRunCancel,
    run_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl WorkflowRunController {
    fn new(driver: Arc<SubAgentWorkflowDriver>, vm_cancel: WorkflowRunCancel) -> Self {
        Self {
            driver,
            vm_cancel,
            run_handle: Mutex::new(None),
        }
    }

    fn set_run_handle(&self, handle: tokio::task::JoinHandle<()>) {
        if let Ok(mut guard) = self.run_handle.lock() {
            *guard = Some(handle);
        }
    }

    fn cancel(&self) {
        self.vm_cancel.cancel();
        self.driver.force_cancel_all();
        if let Ok(mut guard) = self.run_handle.lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct WorkflowRunSummary {
    run_id: String,
    status: WorkflowRunStatus,
    started_at_ms: u64,
    completed_at_ms: Option<u64>,
    source_path: Option<PathBuf>,
    workflow_id: Option<String>,
    workflow_goal: Option<String>,
    token_budget: Option<u64>,
    child_count: usize,
    schema_error_count: usize,
    progress_count: usize,
    last_progress: Option<String>,
    event_count: usize,
    last_event_type: Option<String>,
    leaf_count: usize,
    branch_count: usize,
    control_count: usize,
    execution_status: Option<IrWorkflowRunStatus>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowSchemaError {
    task_id: String,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowUiEvent {
    at_ms: u64,
    #[serde(flatten)]
    kind: WorkflowUiEventKind,
}

impl WorkflowUiEvent {
    fn new(kind: WorkflowUiEventKind) -> Self {
        Self {
            at_ms: now_ms(),
            kind,
        }
    }

    fn at(at_ms: u64, kind: WorkflowUiEventKind) -> Self {
        Self { at_ms, kind }
    }

    fn event_type(&self) -> &'static str {
        self.kind.event_type()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WorkflowUiEventKind {
    RunStarted {
        workflow_id: Option<String>,
        workflow_goal: Option<String>,
        source_path: Option<PathBuf>,
        token_budget: Option<u64>,
    },
    RunCompleted {
        status: WorkflowRunStatus,
        error: Option<String>,
    },
    RunCancelled {
        reason: String,
    },
    PhaseStarted {
        title: String,
    },
    TaskStarted(Box<WorkflowTaskStartedEvent>),
    TaskCompleted {
        task_id: String,
        status: IrWorkflowRunStatus,
    },
    TaskSchemaValidationFailed {
        task_id: String,
        message: String,
    },
    BudgetUpdated {
        total: Option<u64>,
        spent: u64,
        remaining: Option<u64>,
    },
    Log {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowTaskStartedEvent {
    task_id: String,
    label: Option<String>,
    /// Fleet role declared on the step, if any (#4177).
    role: Option<String>,
    profile: Option<String>,
    model: Option<String>,
    strength: Option<String>,
    thinking: Option<String>,
    /// Resolved fleet role after roster lookup (#4177).
    resolved_role: Option<String>,
    /// Resolved AgentProfile id after fleet resolution (#4177).
    resolved_profile: Option<String>,
    resolved_provider: String,
    resolved_model: String,
    route_source: String,
    worktree: bool,
    workspace: Option<PathBuf>,
    git_branch: Option<String>,
    parent_task_id: Option<String>,
    depth: u32,
    /// Workflow run that admitted this child (#4119).
    workflow_run_id: Option<String>,
    /// Phase title/id active (or declared on the task) at spawn (#4119).
    workflow_phase_id: Option<String>,
    /// Typed task label — UI must prefer this over prompt text (#4119).
    workflow_task_label: Option<String>,
    /// 0-based admission order among children of this run (#4119).
    workflow_child_index: Option<u32>,
}

impl WorkflowUiEventKind {
    fn event_type(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::RunCompleted { .. } => "run_completed",
            Self::RunCancelled { .. } => "run_cancelled",
            Self::PhaseStarted { .. } => "phase_started",
            Self::TaskStarted(_) => "task_started",
            Self::TaskCompleted { .. } => "task_completed",
            Self::TaskSchemaValidationFailed { .. } => "task_schema_validation_failed",
            Self::BudgetUpdated { .. } => "budget_updated",
            Self::Log { .. } => "log",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowRunRecord {
    run_id: String,
    status: WorkflowRunStatus,
    started_at_ms: u64,
    completed_at_ms: Option<u64>,
    source_path: Option<PathBuf>,
    workflow_id: Option<String>,
    workflow_goal: Option<String>,
    token_budget: Option<u64>,
    child_ids: Vec<String>,
    progress: Vec<String>,
    #[serde(default)]
    events: Vec<WorkflowUiEvent>,
    schema_errors: Vec<WorkflowSchemaError>,
    result: Option<Value>,
    execution: Option<IrWorkflowExecution>,
    error: Option<String>,
    #[serde(default)]
    verify_on_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification: Option<Value>,
    /// Durable elevated-plan approval receipt for audit (#4126).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan_approval: Option<WorkflowPlanApprovalReceipt>,
}

impl WorkflowRunRecord {
    fn new(
        run_id: String,
        source_path: Option<PathBuf>,
        token_budget: Option<u64>,
        spec: Option<&WorkflowSpec>,
    ) -> Self {
        Self {
            run_id,
            status: WorkflowRunStatus::Running,
            started_at_ms: now_ms(),
            completed_at_ms: None,
            source_path,
            workflow_id: spec.and_then(|spec| spec.id.clone()),
            workflow_goal: spec.map(|spec| spec.goal.clone()),
            token_budget,
            child_ids: Vec::new(),
            progress: Vec::new(),
            events: Vec::new(),
            schema_errors: Vec::new(),
            result: None,
            execution: None,
            error: None,
            verify_on_complete: false,
            verification: None,
            plan_approval: None,
        }
    }

    fn summary(&self) -> WorkflowRunSummary {
        WorkflowRunSummary {
            run_id: self.run_id.clone(),
            status: self.status,
            started_at_ms: self.started_at_ms,
            completed_at_ms: self.completed_at_ms,
            source_path: self.source_path.clone(),
            workflow_id: self.workflow_id.clone(),
            workflow_goal: self.workflow_goal.clone(),
            token_budget: self.token_budget,
            child_count: self.child_ids.len(),
            schema_error_count: self.schema_errors.len(),
            progress_count: self.progress.len(),
            last_progress: self.progress.last().cloned(),
            event_count: self.events.len(),
            last_event_type: self
                .events
                .last()
                .map(|event| event.event_type().to_string()),
            leaf_count: self
                .execution
                .as_ref()
                .map(|execution| execution.leaf_results.len())
                .unwrap_or_default(),
            branch_count: self
                .execution
                .as_ref()
                .map(|execution| execution.branch_results.len())
                .unwrap_or_default(),
            control_count: self
                .execution
                .as_ref()
                .map(|execution| execution.control_node_results.len())
                .unwrap_or_default(),
            execution_status: self.execution.as_ref().map(|execution| execution.status),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WorkflowRunStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowAction {
    Start,
    Run,
    Status,
    Cancel,
}

fn parse_workflow_action(input: &Value) -> Result<WorkflowAction, ToolError> {
    let Some(action) = optional_str(input, "action") else {
        return Ok(WorkflowAction::Start);
    };
    match action.trim().to_ascii_lowercase().as_str() {
        "" | "start" | "spawn" => Ok(WorkflowAction::Start),
        "run" | "wait" => Ok(WorkflowAction::Run),
        "status" | "list" | "inspect" => Ok(WorkflowAction::Status),
        "cancel" | "stop" | "abort" => Ok(WorkflowAction::Cancel),
        other => Err(ToolError::invalid_input(format!(
            "Invalid workflow action '{other}'. Use start, run, status, or cancel."
        ))),
    }
}

#[async_trait]
impl ToolSpec for WorkflowTool {
    fn name(&self) -> &'static str {
        "workflow"
    }

    fn description(&self) -> &'static str {
        concat!(
            "Start, run, inspect, or cancel a Workflow. Workflows execute deterministic JS with args, phase/log progress, and task(...) calls that dispatch real sub-agents through Fleet/sub-agent scheduling. ",
            "Provide exactly one of script, source_path, or plan (structured planner JSON). ",
            "Use action=start for detached orchestration and action=status with run_id to inspect progress. Use action=run when the model needs the final result before continuing."
        )
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "run", "status", "cancel"],
                    "description": "start (default) launches a Workflow in the background. run waits for completion. status lists runs or inspects run_id. cancel stops a run and its child agents."
                },
                "run_id": {
                    "type": "string",
                    "description": "Workflow run id for action=status or action=cancel."
                },
                "script": {
                    "type": "string",
                    "description": "Workflow JS source. The runtime provides args, task(...), parallel(...), pipeline(...), log(...), phase(...), and budget."
                },
                "source_path": {
                    "type": "string",
                    "description": "Path to a .workflow.js script inside the workspace. Use instead of script for checked-in workflows."
                },
                "plan": {
                    "type": "object",
                    "description": "Structured planner plan JSON (#4124). Alternative to script/source_path. Accepts goal, risk, max_children, token_budget, phases[], and/or children[] (or IR nodes). Lowered to Workflow JS with parallel() partial-success semantics."
                },
                "args": {
                    "description": "JSON value exposed to the script as args. Defaults to null."
                },
                "token_budget": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional shared Workflow budget hint and default child token budget ceiling."
                },
                "wait": {
                    "type": "boolean",
                    "description": "For action=start, wait for completion instead of returning immediately."
                },
                "verify": {
                    "type": "boolean",
                    "default": false,
                    "description": "After a successful workflow completion, run quick workspace verifier gates (auto/quick profile)."
                }
            },
            "required": [],
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
        // Default posture: elevated starts require approval. Concrete inputs
        // refine this via `approval_requirement_for` (#4126).
        ApprovalRequirement::Required
    }

    fn approval_requirement_for(&self, input: &Value) -> ApprovalRequirement {
        // Product defaults for [workflow] when the tool has no live Config
        // handle. YOLO/bypass still short-circuit upstream of this check.
        let config = codewhale_config::WorkflowConfigToml::default();
        workflow_approval_requirement_for(input, &config)
    }

    fn starts_detached_for(&self, input: &Value) -> bool {
        matches!(parse_workflow_action(input), Ok(WorkflowAction::Start))
            && !optional_bool(input, "wait", false)
    }

    fn supports_parallel_for(&self, input: &Value) -> bool {
        matches!(parse_workflow_action(input), Ok(WorkflowAction::Status))
    }

    fn is_read_only_for(&self, input: &Value) -> bool {
        matches!(parse_workflow_action(input), Ok(WorkflowAction::Status))
    }

    async fn execute(&self, input: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let state = shared_workflow_state(&context.workspace);
        match parse_workflow_action(&input)? {
            WorkflowAction::Start => {
                let wait = optional_bool(&input, "wait", false);
                start_workflow(
                    input,
                    context,
                    self.manager.clone(),
                    self.runtime.clone(),
                    state,
                    wait,
                )
                .await
            }
            WorkflowAction::Run => {
                start_workflow(
                    input,
                    context,
                    self.manager.clone(),
                    self.runtime.clone(),
                    state,
                    true,
                )
                .await
            }
            WorkflowAction::Status => status_workflow(input, state),
            WorkflowAction::Cancel => cancel_workflow(input, state).await,
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_workflow(
    input: Value,
    context: &ToolContext,
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    state: Arc<WorkflowWorkspaceState>,
    wait: bool,
) -> Result<ToolResult, ToolError> {
    let source = workflow_source(&input, context)?;
    let args = input.get("args").cloned().unwrap_or(Value::Null);
    let token_budget = optional_u64(&input, "token_budget", 0);
    let token_budget = (token_budget > 0).then_some(token_budget);
    let verify_on_complete = optional_bool(&input, "verify", false);
    let run_id = format!("workflow_{}", &Uuid::new_v4().to_string()[..8]);

    // Capture the approved plan envelope for audit/receipt (#4126). Reaching
    // execute means the approval gate already passed (or YOLO/auto-start).
    let workflow_cfg = codewhale_config::WorkflowConfigToml::default();
    let summary = source
        .spec
        .as_ref()
        .map(|spec| analyze_workflow_spec(spec, token_budget, &workflow_cfg))
        .unwrap_or_else(|| analyze_workflow_plan_approval_with_config(&input, &workflow_cfg));
    let approval_decision = if summary.is_read_only_envelope() {
        "auto_read_only"
    } else {
        "approved"
    };
    let plan_approval = summary.to_receipt(approval_decision, now_ms());

    {
        let mut runs_guard = lock_mutex(&state.runs)?;
        let mut record = WorkflowRunRecord::new(
            run_id.clone(),
            source.path.clone(),
            token_budget,
            source.spec.as_ref(),
        );
        record.verify_on_complete = verify_on_complete;
        record.plan_approval = Some(plan_approval.clone());
        let started = WorkflowUiEvent::at(
            record.started_at_ms,
            WorkflowUiEventKind::RunStarted {
                workflow_id: record.workflow_id.clone(),
                workflow_goal: record.workflow_goal.clone(),
                source_path: record.source_path.clone(),
                token_budget: record.token_budget,
            },
        );
        record.events.push(started.clone());
        runs_guard.insert(run_id.clone(), record.clone());
        state.record_snapshot(&record);
        // #4122: emit RunStarted immediately so the panel + history card open
        // before the first task/phase (including wait:false fire-and-forget).
        if let Some(tx) = runtime.event_tx.as_ref() {
            if let Ok(mut value) = serde_json::to_value(&started) {
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("run_id".to_string(), json!(run_id));
                }
                let _ = tx.try_send(Event::WorkflowUi {
                    run_id: run_id.clone(),
                    event: value,
                });
            }
        }
    }

    let driver = SubAgentWorkflowDriver::new(
        run_id.clone(),
        manager,
        runtime,
        state.clone(),
        token_budget,
    );
    let vm_cancel = WorkflowRunCancel::new();
    let controller = Arc::new(WorkflowRunController::new(
        driver.clone(),
        vm_cancel.clone(),
    ));
    {
        let mut controllers_guard = lock_mutex(&state.controllers)?;
        controllers_guard.insert(run_id.clone(), controller.clone());
    }

    let run = run_workflow_vm(
        run_id.clone(),
        source.source,
        source.spec,
        args,
        driver,
        state.clone(),
        context.clone(),
        vm_cancel,
    );
    if wait {
        run.await;
    } else {
        let handle = spawn_supervised("workflow-run", std::panic::Location::caller(), run);
        controller.set_run_handle(handle);
    }

    workflow_result_for(&run_id, state)
}

fn status_workflow(
    input: Value,
    state: Arc<WorkflowWorkspaceState>,
) -> Result<ToolResult, ToolError> {
    if let Some(run_id) = optional_str(&input, "run_id") {
        return workflow_result_for(run_id, state);
    }
    let mut summaries = {
        let runs_guard = lock_mutex(&state.runs)?;
        runs_guard
            .values()
            .map(WorkflowRunRecord::summary)
            .collect::<Vec<_>>()
    };
    summaries.sort_by_key(|record| record.started_at_ms);
    ToolResult::json(&json!({
        "action": "status",
        "count": summaries.len(),
        "runs": summaries,
    }))
    .map_err(|err| ToolError::execution_failed(err.to_string()))
}

async fn cancel_workflow(
    input: Value,
    state: Arc<WorkflowWorkspaceState>,
) -> Result<ToolResult, ToolError> {
    let run_id =
        optional_str(&input, "run_id").ok_or_else(|| ToolError::missing_field("run_id"))?;
    let controller = {
        let mut controllers_guard = lock_mutex(&state.controllers)?;
        controllers_guard.remove(run_id)
    };
    if let Some(controller) = controller {
        controller.cancel();
    }
    let snapshot = {
        let mut runs_guard = lock_mutex(&state.runs)?;
        let record = runs_guard.get_mut(run_id).ok_or_else(|| {
            ToolError::invalid_input(format!("Unknown workflow run_id '{run_id}'"))
        })?;
        record.status = WorkflowRunStatus::Cancelled;
        record.completed_at_ms = Some(now_ms());
        let reason = "cancelled by workflow tool".to_string();
        record.error = Some(reason.clone());
        record
            .events
            .push(WorkflowUiEvent::new(WorkflowUiEventKind::RunCancelled {
                reason,
            }));
        record.clone()
    };
    state.record_snapshot(&snapshot);
    workflow_result_for(run_id, state)
}

// Pre-existing spawn signature that grew `vm_cancel` for the cancel-interrupt
// wiring; the args mirror one workflow run's context and are consumed once.
#[allow(clippy::too_many_arguments)]
async fn run_workflow_vm(
    run_id: String,
    source: String,
    spec: Option<WorkflowSpec>,
    args: Value,
    driver: Arc<SubAgentWorkflowDriver>,
    state: Arc<WorkflowWorkspaceState>,
    context: ToolContext,
    vm_cancel: WorkflowRunCancel,
) {
    let result = WorkflowVm::new()
        .run_script_with_cancel(&source, args, driver.clone(), vm_cancel)
        .await;
    let mut status = WorkflowRunStatus::Completed;
    let mut output = None;
    let mut error = None;
    match result {
        Ok(value) => output = Some(value),
        Err(err) => {
            status = WorkflowRunStatus::Failed;
            error = Some(err.to_string());
        }
    }
    let snapshot = {
        let mut runs_guard = match state.runs.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let Some(record) = runs_guard.get_mut(&run_id) else {
            return;
        };
        if record.status != WorkflowRunStatus::Cancelled {
            record.status = status;
            record.result = output;
            record.error = error.clone();
            record.execution = spec.as_ref().map(|spec| {
                execution_from_declarative_spec(spec, driver.task_records_snapshot(), status)
            });
            record.completed_at_ms = Some(now_ms());
        }
        record.clone()
    };
    let verify_on_complete = state
        .runs
        .lock()
        .ok()
        .and_then(|guard| guard.get(&run_id).map(|record| record.verify_on_complete))
        .unwrap_or(false);
    if status == WorkflowRunStatus::Completed && verify_on_complete {
        match run_workflow_completion_gates(&context).await {
            Ok(verification) => {
                if let Ok(mut runs_guard) = state.runs.lock()
                    && let Some(record) = runs_guard.get_mut(&run_id)
                {
                    record.verification = Some(verification);
                }
            }
            Err(err) => {
                if let Ok(mut runs_guard) = state.runs.lock()
                    && let Some(record) = runs_guard.get_mut(&run_id)
                {
                    record.status = WorkflowRunStatus::Failed;
                    record.error = Some(format!("verification gates failed: {err}"));
                }
            }
        }
    }
    let final_budget = driver.current_budget_snapshot();
    let snapshot = state
        .runs
        .lock()
        .ok()
        .and_then(|mut guard| {
            let record = guard.get_mut(&run_id)?;
            if record.status != WorkflowRunStatus::Cancelled {
                let budget_event = WorkflowUiEvent::new(budget_event_kind(final_budget));
                let completed = WorkflowUiEvent::new(WorkflowUiEventKind::RunCompleted {
                    status: record.status,
                    error: record.error.clone(),
                });
                record.events.push(budget_event.clone());
                record.events.push(completed.clone());
                // Live stream terminal events even when recorded outside the
                // driver helper (completion path).
                driver.emit_ui_event(&budget_event);
                driver.emit_ui_event(&completed);
            }
            Some(record.clone())
        })
        .unwrap_or(snapshot);
    state.record_snapshot(&snapshot);
    if let Ok(mut controllers_guard) = state.controllers.lock() {
        controllers_guard.remove(&run_id);
    }
}

fn workflow_result_for(
    run_id: &str,
    state: Arc<WorkflowWorkspaceState>,
) -> Result<ToolResult, ToolError> {
    let record = {
        let runs_guard = lock_mutex(&state.runs)?;
        runs_guard.get(run_id).cloned().ok_or_else(|| {
            ToolError::invalid_input(format!("Unknown workflow run_id '{run_id}'"))
        })?
    };
    let mut result =
        ToolResult::json(&record).map_err(|err| ToolError::execution_failed(err.to_string()))?;
    let summary = record.summary();
    result.metadata = Some(json!({
        "run_id": summary.run_id,
        "status": summary.status,
        "terminal": summary.status != WorkflowRunStatus::Running,
        "child_count": summary.child_count,
        "schema_error_count": summary.schema_error_count,
        "event_count": summary.event_count,
        "last_event_type": summary.last_event_type,
        "leaf_count": summary.leaf_count,
        "branch_count": summary.branch_count,
        "control_count": summary.control_count,
        "execution_status": summary.execution_status,
        // #4126: durable plan-approval receipt for audit/receipt consumers.
        "plan_approval": record.plan_approval,
    }));
    Ok(result)
}

#[derive(Debug)]
struct WorkflowSource {
    source: String,
    path: Option<PathBuf>,
    spec: Option<WorkflowSpec>,
}

fn workflow_source(input: &Value, context: &ToolContext) -> Result<WorkflowSource, ToolError> {
    let script = optional_str(input, "script")
        .or_else(|| optional_str(input, "source"))
        .map(str::to_string);
    let source_path = optional_str(input, "source_path").or_else(|| optional_str(input, "path"));
    let plan = input.get("plan").filter(|value| !value.is_null());

    let provided = [
        script.as_ref().is_some_and(|s| !s.trim().is_empty()),
        source_path.is_some(),
        plan.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if provided > 1 {
        return Err(ToolError::invalid_input(
            "Use exactly one of script, source_path, or plan",
        ));
    }

    match (script, source_path, plan) {
        (Some(source), None, None) if !source.trim().is_empty() => {
            workflow_source_from_raw(source, None)
        }
        (None, Some(path), None) => read_workflow_source_path(path, context),
        (None, None, Some(plan_value)) => workflow_source_from_plan(plan_value),
        _ => Err(ToolError::missing_field("script")),
    }
}

/// Planner-to-workflow structured launch path (#4124).
///
/// Accepts product-shaped plans (`goal` + `phases`/`children`) or IR-shaped
/// plans (`goal` + `nodes`), validates them, and lowers to imperative JS that
/// uses `parallel()` (partial success) rather than raw `Promise.all()`.
fn workflow_source_from_plan(plan_value: &Value) -> Result<WorkflowSource, ToolError> {
    let spec = structured_plan_to_workflow_spec(plan_value)?;
    let lowered = lower_declarative_workflow_to_imperative_js(&spec)?;
    Ok(WorkflowSource {
        source: lowered,
        path: None,
        spec: Some(spec),
    })
}

#[derive(Debug, Deserialize)]
struct StructuredWorkflowPlan {
    goal: String,
    #[serde(default)]
    risk: Option<String>,
    #[serde(default)]
    max_children: Option<usize>,
    #[serde(default)]
    token_budget: Option<u64>,
    #[serde(default)]
    phases: Vec<StructuredPlanPhase>,
    #[serde(default)]
    children: Vec<StructuredPlanChild>,
    /// Escape hatch: full Workflow IR nodes (kind/spec or JS authoring shapes).
    #[serde(default)]
    nodes: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct StructuredPlanPhase {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    parallel: Option<bool>,
    #[serde(default)]
    children: Vec<StructuredPlanChild>,
}

#[derive(Debug, Deserialize)]
struct StructuredPlanChild {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(alias = "description")]
    prompt: String,
    #[serde(default, alias = "type", alias = "agent_type")]
    agent_type: Option<String>,
    /// Fleet role name (#4177). Preferred step identity; resolved via roster.
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    profile: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

fn structured_plan_to_workflow_spec(plan_value: &Value) -> Result<WorkflowSpec, ToolError> {
    if !plan_value.is_object() {
        return Err(ToolError::invalid_input(
            "Workflow plan must be a JSON object with goal and phases/children (or nodes)",
        ));
    }

    let plan: StructuredWorkflowPlan =
        serde_json::from_value(plan_value.clone()).map_err(|err| {
            ToolError::invalid_input(format!("Invalid structured Workflow plan: {err}"))
        })?;

    let goal = plan.goal.trim();
    if goal.is_empty() {
        return Err(ToolError::invalid_input(
            "Workflow plan goal must be a non-empty string",
        ));
    }

    // IR / declarative nodes escape hatch: re-parse as workflow({...}) object.
    if let Some(nodes) = plan.nodes.as_ref() {
        if !nodes.is_array() {
            return Err(ToolError::invalid_input(
                "Workflow plan.nodes must be an array of workflow nodes",
            ));
        }
        let mut object = plan_value.clone();
        if let Some(obj) = object.as_object_mut() {
            obj.insert("goal".to_string(), Value::String(goal.to_string()));
            if let Some(token_budget) = plan.token_budget {
                let mut budget = obj.get("budget").cloned().unwrap_or_else(|| json!({}));
                if let Some(budget_obj) = budget.as_object_mut() {
                    budget_obj.insert("max_tokens".to_string(), json!(token_budget));
                }
                obj.insert("budget".to_string(), budget);
            }
        }
        let wrapped = format!("workflow({});", object);
        return compile_javascript_workflow("<structured plan>", &wrapped).map_err(|err| {
            ToolError::invalid_input(format!("Invalid structured Workflow plan nodes: {err}"))
        });
    }

    let default_mode = plan_risk_to_mode(plan.risk.as_deref())?;
    let mut nodes = Vec::new();

    if !plan.phases.is_empty() {
        for (phase_index, phase) in plan.phases.iter().enumerate() {
            let phase_id = phase
                .id
                .as_deref()
                .or(phase.title.as_deref())
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("phase-{}", phase_index + 1));
            let children = plan_children_to_leaves(
                &phase.children,
                default_mode,
                plan.token_budget,
                &phase_id,
            )?;
            if children.is_empty() {
                return Err(ToolError::invalid_input(format!(
                    "Workflow plan phase '{phase_id}' must declare at least one child"
                )));
            }
            let parallel = phase.parallel.unwrap_or(children.len() > 1);
            if parallel && children.len() > 1 {
                nodes.push(WorkflowNode::BranchSet(BranchSpec {
                    id: phase_id,
                    description: phase.title.clone(),
                    parallel: true,
                    budget: BudgetSpec {
                        max_tokens: plan.token_budget,
                        ..BudgetSpec::default()
                    },
                    permissions: Default::default(),
                    model_policy: Default::default(),
                    children: children.into_iter().map(WorkflowNode::Leaf).collect(),
                }));
            } else if children.len() == 1 {
                nodes.push(WorkflowNode::Leaf(
                    children.into_iter().next().expect("one child"),
                ));
            } else {
                nodes.push(WorkflowNode::Sequence(SequenceSpec {
                    id: phase_id,
                    children: children.into_iter().map(WorkflowNode::Leaf).collect(),
                }));
            }
        }
    } else if !plan.children.is_empty() {
        let children =
            plan_children_to_leaves(&plan.children, default_mode, plan.token_budget, "plan")?;
        if children.len() == 1 {
            nodes.push(WorkflowNode::Leaf(
                children.into_iter().next().expect("one child"),
            ));
        } else {
            nodes.push(WorkflowNode::BranchSet(BranchSpec {
                id: "plan".to_string(),
                description: Some(goal.to_string()),
                parallel: true,
                budget: BudgetSpec {
                    max_tokens: plan.token_budget,
                    ..BudgetSpec::default()
                },
                permissions: Default::default(),
                model_policy: Default::default(),
                children: children.into_iter().map(WorkflowNode::Leaf).collect(),
            }));
        }
    } else {
        return Err(ToolError::invalid_input(
            "Workflow plan must include phases, children, or nodes",
        ));
    }

    let mut total_children = 0usize;
    count_plan_leaves(&nodes, &mut total_children);
    if let Some(max_children) = plan.max_children
        && total_children > max_children
    {
        return Err(ToolError::invalid_input(format!(
            "Workflow plan declares {total_children} children which exceeds max_children={max_children}"
        )));
    }

    Ok(WorkflowSpec {
        id: None,
        goal: goal.to_string(),
        description: plan.risk.clone(),
        budget: BudgetSpec {
            max_tokens: plan.token_budget,
            ..BudgetSpec::default()
        },
        permissions: Default::default(),
        model_policy: Default::default(),
        promotion_policy: Default::default(),
        nodes,
    })
}

fn plan_risk_to_mode(risk: Option<&str>) -> Result<TaskMode, ToolError> {
    match risk.map(str::trim).filter(|s| !s.is_empty()) {
        None | Some("read_only") | Some("readonly") | Some("low") | Some("safe") => {
            Ok(TaskMode::ReadOnly)
        }
        Some("writes") | Some("write") | Some("read_write") | Some("readwrite")
        | Some("medium") => Ok(TaskMode::ReadWrite),
        Some("elevated") | Some("high") | Some("shell") | Some("network") => {
            // Elevated risk still launches as read_write; approval gates (#4126)
            // consume the risk string via plan description.
            Ok(TaskMode::ReadWrite)
        }
        Some(other) => Err(ToolError::invalid_input(format!(
            "Invalid plan risk '{other}'. Use read_only, writes, or elevated."
        ))),
    }
}

fn plan_children_to_leaves(
    children: &[StructuredPlanChild],
    default_mode: TaskMode,
    token_budget: Option<u64>,
    phase_id: &str,
) -> Result<Vec<LeafSpec>, ToolError> {
    if children.is_empty() {
        return Ok(Vec::new());
    }
    let mut leaves = Vec::with_capacity(children.len());
    for (index, child) in children.iter().enumerate() {
        let prompt = child.prompt.trim();
        if prompt.is_empty() {
            return Err(ToolError::invalid_input(format!(
                "Workflow plan child {} in phase '{phase_id}' must have a non-empty prompt",
                index + 1
            )));
        }
        let id = child
            .id
            .as_deref()
            .or(child.label.as_deref())
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("{phase_id}-child-{}", index + 1));
        let agent_type = parse_plan_agent_type(child.agent_type.as_deref())?;
        let mode = match child.mode.as_deref().map(str::trim) {
            None | Some("") => default_mode,
            Some("read_only") | Some("readonly") => TaskMode::ReadOnly,
            Some("read_write") | Some("readwrite") | Some("writes") | Some("write") => {
                TaskMode::ReadWrite
            }
            Some(other) => {
                return Err(ToolError::invalid_input(format!(
                    "Invalid plan child mode '{other}' on '{id}'. Use read_only or read_write."
                )));
            }
        };
        let role = child
            .role
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .map(|r| r.to_ascii_lowercase());
        let profile = child
            .profile
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_ascii_lowercase());
        leaves.push(LeafSpec {
            id,
            prompt: prompt.to_string(),
            agent_type,
            role,
            profile,
            mode,
            isolation: Default::default(),
            file_scope: Vec::new(),
            depends_on_results: Vec::new(),
            budget: BudgetSpec {
                max_tokens: token_budget,
                ..BudgetSpec::default()
            },
            permissions: Default::default(),
            model_policy: Default::default(),
        });
    }
    Ok(leaves)
}

fn parse_plan_agent_type(raw: Option<&str>) -> Result<AgentType, ToolError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(AgentType::General),
        Some("general") | Some("worker") | Some("delegate") => Ok(AgentType::General),
        Some("explore") | Some("scout") => Ok(AgentType::Explore),
        Some("plan") | Some("planner") => Ok(AgentType::Plan),
        Some("review") | Some("reviewer") => Ok(AgentType::Review),
        Some("implementer") | Some("builder") | Some("implement") => Ok(AgentType::Implementer),
        Some("verifier") | Some("verify") => Ok(AgentType::Verifier),
        Some(other) => Err(ToolError::invalid_input(format!(
            "Invalid plan child type '{other}'. Use general, explore, plan, review, implementer, or verifier."
        ))),
    }
}

fn count_plan_leaves(nodes: &[WorkflowNode], total: &mut usize) {
    for node in nodes {
        match node {
            WorkflowNode::Leaf(_) => *total += 1,
            WorkflowNode::BranchSet(spec) => count_plan_leaves(&spec.children, total),
            WorkflowNode::Sequence(spec) => count_plan_leaves(&spec.children, total),
            WorkflowNode::Reduce(_)
            | WorkflowNode::TeacherReview(_)
            | WorkflowNode::LoopUntil(_)
            | WorkflowNode::Cond(_)
            | WorkflowNode::Expand(_) => {}
        }
    }
}

fn read_workflow_source_path(
    path: &str,
    context: &ToolContext,
) -> Result<WorkflowSource, ToolError> {
    let raw = Path::new(path);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        context.workspace.join(raw)
    };
    let canonical = joined.canonicalize().map_err(|err| {
        ToolError::invalid_input(format!(
            "Failed to resolve workflow source_path '{path}': {err}"
        ))
    })?;
    if !context.trust_mode {
        let workspace = context
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| context.workspace.clone());
        if !canonical.starts_with(&workspace) {
            return Err(ToolError::permission_denied(format!(
                "workflow source_path must stay inside the workspace: {}",
                canonical.display()
            )));
        }
    }
    let source = std::fs::read_to_string(&canonical).map_err(|err| {
        ToolError::execution_failed(format!(
            "Failed to read workflow source_path '{}': {err}",
            canonical.display()
        ))
    })?;
    workflow_source_from_raw(source, Some(canonical))
}

fn workflow_source_from_raw(
    source: String,
    path: Option<PathBuf>,
) -> Result<WorkflowSource, ToolError> {
    let adapted = adapt_workflow_source(&source, path.as_deref())?;
    Ok(WorkflowSource {
        source: adapted.source,
        path,
        spec: adapted.spec,
    })
}

struct AdaptedWorkflowSource {
    source: String,
    spec: Option<WorkflowSpec>,
}

fn adapt_workflow_source(
    source: &str,
    path: Option<&Path>,
) -> Result<AdaptedWorkflowSource, ToolError> {
    if !looks_like_declarative_workflow(source) {
        return Ok(AdaptedWorkflowSource {
            source: source.to_string(),
            spec: None,
        });
    }

    let identifier = path
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<inline workflow>".to_string());
    let extension = path
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let spec = if extension.eq_ignore_ascii_case("ts") {
        compile_typescript_workflow(&identifier, source)
    } else {
        compile_javascript_workflow(&identifier, source)
    }
    .map_err(|err| {
        ToolError::invalid_input(format!(
            "Failed to compile declarative Workflow source '{identifier}': {err}"
        ))
    })?;

    let lowered = lower_declarative_workflow_to_imperative_js(&spec)?;
    Ok(AdaptedWorkflowSource {
        source: lowered,
        spec: Some(spec),
    })
}

fn looks_like_declarative_workflow(source: &str) -> bool {
    // Match a top-level `workflow(...)` / `export default workflow(...)` call on
    // any line, ignoring leading indentation, so an indented (non-column-0)
    // declarative call is still recognized rather than misrun as an imperative
    // script (#dogfood 0.8.67).
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("workflow(") || trimmed.starts_with("export default workflow(")
    })
}

fn lower_declarative_workflow_to_imperative_js(spec: &WorkflowSpec) -> Result<String, ToolError> {
    let mut lowerer = DeclarativeWorkflowLowerer::default();
    lowerer.line("\"use strict\";");
    lowerer.line("const __results = {};");
    lowerer.line(format!(
        "phase({});",
        js_string(&format!("workflow: {}", spec.goal))
    ));
    for node in &spec.nodes {
        lowerer.lower_node(node, None)?;
    }
    lowerer.line("return __results;");
    Ok(lowerer.finish())
}

#[derive(Default)]
struct DeclarativeWorkflowLowerer {
    source: String,
    next_var: usize,
}

impl DeclarativeWorkflowLowerer {
    fn finish(self) -> String {
        self.source
    }

    fn line(&mut self, line: impl AsRef<str>) {
        self.source.push_str(line.as_ref());
        self.source.push('\n');
    }

    fn next_temp(&mut self, prefix: &str) -> String {
        let value = format!("__{prefix}_{}", self.next_var);
        self.next_var += 1;
        value
    }

    fn lower_node(&mut self, node: &WorkflowNode, phase: Option<&str>) -> Result<(), ToolError> {
        match node {
            WorkflowNode::Leaf(spec) => self.lower_leaf(spec, phase, /* parallel */ false),
            WorkflowNode::BranchSet(spec) => self.lower_branch(spec),
            WorkflowNode::Sequence(spec) => self.lower_sequence(spec),
            WorkflowNode::Reduce(spec) => self.lower_reduce(spec),
            WorkflowNode::TeacherReview(_) => Err(unsupported_declarative_node("teacher_review")),
            WorkflowNode::LoopUntil(_) => Err(unsupported_declarative_node("loop_until")),
            WorkflowNode::Cond(_) => Err(unsupported_declarative_node("cond")),
            WorkflowNode::Expand(_) => Err(unsupported_declarative_node("expand")),
        }
    }

    fn lower_leaf(
        &mut self,
        spec: &LeafSpec,
        phase: Option<&str>,
        parallel: bool,
    ) -> Result<(), ToolError> {
        self.line(format!(
            "__results[{}] = await task({});",
            js_string(&spec.id),
            leaf_task_options_expression(spec, phase, parallel)?
        ));
        Ok(())
    }

    fn lower_branch(&mut self, spec: &BranchSpec) -> Result<(), ToolError> {
        self.line(format!("phase({});", js_string(&spec.id)));
        if spec.parallel {
            let mut leaves = Vec::new();
            for child in &spec.children {
                let WorkflowNode::Leaf(leaf) = child else {
                    return Err(ToolError::invalid_input(format!(
                        "Declarative Workflow adapter only supports leaf children inside parallel branch '{}'",
                        spec.id
                    )));
                };
                leaves.push(leaf);
            }
            // #4124: use Workflow `parallel()` (all-settled / partial success)
            // instead of raw Promise.all, which aborts siblings on first failure.
            let temp = self.next_temp("parallel");
            self.line(format!("const {temp} = await parallel(["));
            for leaf in &leaves {
                // Parallel write-capable children default to worktree isolation
                // (#4120) unless the plan explicitly sets isolation: shared.
                self.line(format!(
                    "  () => task({}),",
                    leaf_task_options_expression(leaf, Some(&spec.id), /* parallel */ true)?
                ));
            }
            self.line("]);");
            for (index, leaf) in leaves.iter().enumerate() {
                self.line(format!(
                    "__results[{}] = {temp}[{index}];",
                    js_string(&leaf.id)
                ));
            }
            return Ok(());
        }

        for child in &spec.children {
            self.lower_node(child, Some(&spec.id))?;
        }
        Ok(())
    }

    fn lower_sequence(&mut self, spec: &SequenceSpec) -> Result<(), ToolError> {
        self.line(format!("phase({});", js_string(&spec.id)));
        for child in &spec.children {
            self.lower_node(child, Some(&spec.id))?;
        }
        Ok(())
    }

    fn lower_reduce(&mut self, spec: &ReduceSpec) -> Result<(), ToolError> {
        let inputs = result_inputs_expression(&spec.inputs);
        self.line(format!(
            "__results[{}] = await task({});",
            js_string(&spec.id),
            task_options_expression(
                format!(
                    "{} + \"\\n\\nInputs:\\n\" + {inputs}",
                    js_string(&spec.prompt)
                ),
                "general",
                None,
                None,
                false,
                None,
                &spec.id,
                Some("reduce"),
                None,
            )
        ));
        Ok(())
    }
}

fn unsupported_declarative_node(kind: &str) -> ToolError {
    ToolError::invalid_input(format!(
        "Declarative Workflow adapter does not yet support {kind} nodes"
    ))
}

fn leaf_description(spec: &LeafSpec) -> String {
    let mut description = spec.prompt.trim().to_string();
    let mut metadata = Vec::new();
    metadata.push(format!("Workflow leaf id: {}", spec.id));
    metadata.push(format!("Mode: {}", task_mode_name(spec.mode)));
    if !spec.file_scope.is_empty() {
        metadata.push(format!("File scope: {}", spec.file_scope.join(", ")));
    }
    if !spec.depends_on_results.is_empty() {
        metadata.push(format!(
            "Depends on results: {}",
            spec.depends_on_results.join(", ")
        ));
    }
    if spec.budget != BudgetSpec::default() {
        let mut budget = Vec::new();
        if let Some(max_steps) = spec.budget.max_steps {
            budget.push(format!("max_steps={max_steps}"));
        }
        if let Some(timeout_secs) = spec.budget.timeout_secs {
            budget.push(format!("timeout_secs={timeout_secs}"));
        }
        if let Some(max_parallel) = spec.budget.max_parallel {
            budget.push(format!("max_parallel={max_parallel}"));
        }
        if let Some(max_tokens) = spec.budget.max_tokens {
            budget.push(format!("max_tokens={max_tokens}"));
        }
        if !budget.is_empty() {
            metadata.push(format!("Budget: {}", budget.join(", ")));
        }
    }
    if !metadata.is_empty() {
        description.push_str("\n\nWorkflow metadata:\n");
        for item in metadata {
            description.push_str("- ");
            description.push_str(&item);
            description.push('\n');
        }
    }
    description
}

fn leaf_task_options_expression(
    spec: &LeafSpec,
    phase: Option<&str>,
    parallel: bool,
) -> Result<String, ToolError> {
    validate_leaf_runtime_contract(spec)?;
    Ok(task_options_expression(
        leaf_description_expression(spec),
        leaf_subagent_type(spec)?,
        spec.role.as_deref(),
        spec.profile.as_deref(),
        // Parallel write-capable children default to worktree isolation (#4120).
        // Explicit isolation: shared is the approved same-worktree override.
        leaf_wants_worktree(spec, parallel),
        spec.budget.max_tokens,
        &spec.id,
        phase,
        leaf_allowed_tools(spec)?,
    ))
}

fn validate_leaf_runtime_contract(spec: &LeafSpec) -> Result<(), ToolError> {
    if spec.mode == TaskMode::ReadOnly && spec.permissions.allow_write {
        return Err(ToolError::invalid_input(format!(
            "Workflow leaf '{}' is read_only but requests allow_write permissions",
            spec.id
        )));
    }
    if spec.mode == TaskMode::ReadOnly && matches!(spec.agent_type, AgentType::Implementer) {
        return Err(ToolError::invalid_input(format!(
            "Workflow leaf '{}' is read_only but uses implementer agent_type",
            spec.id
        )));
    }
    if spec.mode == TaskMode::ReadWrite
        && matches!(
            spec.agent_type,
            AgentType::Explore | AgentType::Plan | AgentType::Review | AgentType::Verifier
        )
    {
        return Err(ToolError::invalid_input(format!(
            "Workflow leaf '{}' is read_write but uses read-only agent_type {}",
            spec.id,
            agent_type_name(spec.agent_type)
        )));
    }
    if spec.mode == TaskMode::ReadOnly
        && spec
            .permissions
            .allowed_tools
            .iter()
            .any(|tool| is_write_or_shell_tool(tool))
    {
        return Err(ToolError::invalid_input(format!(
            "Workflow leaf '{}' is read_only but requests write/shell allowed_tools",
            spec.id
        )));
    }
    Ok(())
}

fn leaf_description_expression(spec: &LeafSpec) -> String {
    let description = js_string(&leaf_description(spec));
    if spec.depends_on_results.is_empty() {
        return description;
    }
    let inputs = result_inputs_expression(&spec.depends_on_results);
    format!("{description} + \"\\n\\nInputs:\\n\" + {inputs}")
}

fn result_inputs_expression(inputs: &[String]) -> String {
    let entries = inputs
        .iter()
        .map(|input| format!("[{}, __results[{}]]", js_string(input), js_string(input)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "[{entries}].map(([id, value]) => \"--- \" + id + \" ---\\n\" + String(value ?? \"\")).join(\"\\n\\n\")"
    )
}

fn leaf_subagent_type(spec: &LeafSpec) -> Result<&'static str, ToolError> {
    if spec.mode == TaskMode::ReadOnly && spec.agent_type == AgentType::General {
        return Ok("review");
    }
    Ok(agent_type_name(spec.agent_type))
}

fn leaf_allowed_tools(spec: &LeafSpec) -> Result<Option<Vec<String>>, ToolError> {
    if !spec.permissions.allowed_tools.is_empty() {
        return Ok(Some(spec.permissions.allowed_tools.clone()));
    }
    if spec.mode != TaskMode::ReadOnly {
        return Ok(None);
    }
    Ok(Some(
        read_only_allowed_tools(spec.agent_type)
            .iter()
            .map(|tool| (*tool).to_string())
            .collect(),
    ))
}

fn read_only_allowed_tools(agent_type: AgentType) -> &'static [&'static str] {
    match agent_type {
        AgentType::Verifier => &["list_dir", "read_file", "grep_files", "file_search"],
        _ => &["list_dir", "read_file", "grep_files", "file_search"],
    }
}

fn is_write_or_shell_tool(tool: &str) -> bool {
    matches!(
        tool.trim(),
        "write_file"
            | "edit_file"
            | "apply_patch"
            | "exec_shell"
            | "exec_shell_wait"
            | "exec_shell_interact"
            | "exec_wait"
            | "exec_interact"
    )
}

// Pre-existing builder that grew `allowed_tools`; each arg maps 1:1 onto one
// optional field of the generated JS options literal.
#[allow(clippy::too_many_arguments)]
fn task_options_expression(
    description_expr: String,
    subagent_type: &str,
    role: Option<&str>,
    profile: Option<&str>,
    worktree: bool,
    token_budget: Option<u64>,
    label: &str,
    phase: Option<&str>,
    allowed_tools: Option<Vec<String>>,
) -> String {
    let mut fields = vec![
        format!("description: {description_expr}"),
        format!("type: {}", js_string(subagent_type)),
        format!("label: {}", js_string(label)),
    ];
    if let Some(phase) = phase {
        fields.push(format!("phase: {}", js_string(phase)));
    }
    if let Some(role) = role {
        fields.push(format!("role: {}", js_string(role)));
    }
    if let Some(profile) = profile {
        fields.push(format!("profile: {}", js_string(profile)));
    }
    if worktree {
        fields.push("worktree: true".to_string());
    }
    if let Some(token_budget) = token_budget {
        fields.push(format!("tokenBudget: {token_budget}"));
    }
    if let Some(allowed_tools) = allowed_tools {
        fields.push(format!(
            "allowedTools: {}",
            serde_json::to_string(&allowed_tools).expect("serializing tool names cannot fail")
        ));
    }
    format!("{{ {} }}", fields.join(", "))
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing JS string cannot fail")
}

fn agent_type_name(agent_type: AgentType) -> &'static str {
    match agent_type {
        AgentType::General => "general",
        AgentType::Explore => "explore",
        AgentType::Plan => "plan",
        AgentType::Review => "review",
        AgentType::Implementer => "implementer",
        AgentType::Verifier => "verifier",
    }
}

fn task_mode_name(mode: TaskMode) -> &'static str {
    match mode {
        TaskMode::ReadOnly => "read_only",
        TaskMode::ReadWrite => "read_write",
    }
}

#[derive(Debug, Clone)]
struct RuntimeTaskRecord {
    agent_id: String,
    label: Option<String>,
    status: IrWorkflowRunStatus,
    output: Option<String>,
    schema_error: Option<String>,
}

struct SubAgentWorkflowDriver {
    run_id: String,
    manager: SharedSubAgentManager,
    runtime: SubAgentRuntime,
    state: Arc<WorkflowWorkspaceState>,
    completion_tx: mpsc::UnboundedSender<SubAgentCompletion>,
    completion_state: Arc<Mutex<CompletionState>>,
    child_ids: Arc<Mutex<Vec<String>>>,
    /// Monotonic 0-based child admission counter for `workflow_child_index`.
    child_counter: AtomicU32,
    /// Latest `phase(...)` title observed on this run (used when a task omits
    /// an explicit `phase` option).
    current_phase: Mutex<Option<String>>,
    task_records: Arc<Mutex<HashMap<String, RuntimeTaskRecord>>>,
    total_budget: Option<u64>,
    last_budget_event: Arc<Mutex<Option<BudgetSnapshot>>>,
    /// Caps concurrently live `task()` children for this run (product: 16).
    concurrent_gate: Arc<Semaphore>,
    /// Held permits for in-flight children; released on completion/cancel.
    spawn_permits: Mutex<HashMap<String, OwnedSemaphorePermit>>,
}

impl SubAgentWorkflowDriver {
    fn new(
        run_id: String,
        manager: SharedSubAgentManager,
        runtime: SubAgentRuntime,
        state: Arc<WorkflowWorkspaceState>,
        total_budget: Option<u64>,
    ) -> Arc<Self> {
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        let driver = Arc::new(Self {
            run_id,
            manager,
            runtime,
            state,
            completion_tx,
            completion_state: Arc::new(Mutex::new(CompletionState::default())),
            child_ids: Arc::new(Mutex::new(Vec::new())),
            child_counter: AtomicU32::new(0),
            current_phase: Mutex::new(None),
            task_records: Arc::new(Mutex::new(HashMap::new())),
            total_budget,
            last_budget_event: Arc::new(Mutex::new(None)),
            concurrent_gate: Arc::new(Semaphore::new(WORKFLOW_MAX_CONCURRENT.max(1))),
            spawn_permits: Mutex::new(HashMap::new()),
        });
        spawn_completion_pump(driver.clone(), completion_rx);
        driver
    }

    fn force_cancel_all(&self) {
        let ids = self
            .child_ids
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default();
        if let Ok(mut permits) = self.spawn_permits.lock() {
            permits.clear();
        }
        cancel_child_agents(self.manager.clone(), ids);
        if let Ok(mut state) = self.completion_state.lock() {
            for (_, waiter) in state.waiters.drain() {
                let _ = waiter.send(TaskCompletion::Cancelled);
            }
        }
    }

    fn record_child(&self, agent_id: &str) {
        if let Ok(mut ids) = self.child_ids.lock()
            && !ids.iter().any(|id| id == agent_id)
        {
            ids.push(agent_id.to_string());
        }
        if let Ok(mut runs) = self.state.runs.lock()
            && let Some(record) = runs.get_mut(&self.run_id)
            && !record.child_ids.iter().any(|id| id == agent_id)
        {
            record.child_ids.push(agent_id.to_string());
        }
    }

    fn current_budget_snapshot(&self) -> BudgetSnapshot {
        let spent = self
            .manager
            .try_read()
            .ok()
            .map(|manager| manager.budget_spent_for_scope(&self.run_id))
            .unwrap_or(0);
        BudgetSnapshot {
            total: self.total_budget,
            spent,
        }
    }

    fn record_run_event(&self, event: WorkflowUiEvent) {
        let recorded = if let Ok(mut runs) = self.state.runs.lock()
            && let Some(record) = runs.get_mut(&self.run_id)
        {
            record.events.push(event.clone());
            true
        } else {
            false
        };
        if recorded {
            self.state.record_event(&self.run_id, &event);
        }
        // #4122: stream typed events live into the panel + history card.
        self.emit_ui_event(&event);
    }

    /// Publish a flattened WorkflowUiEvent on the engine event bus so the TUI
    /// can hydrate the panel while the tool is still running.
    fn emit_ui_event(&self, event: &WorkflowUiEvent) {
        let Some(tx) = self.runtime.event_tx.as_ref() else {
            return;
        };
        let Ok(mut value) = serde_json::to_value(event) else {
            return;
        };
        if let Some(obj) = value.as_object_mut() {
            obj.insert("run_id".to_string(), json!(self.run_id));
        }
        let _ = tx.try_send(Event::WorkflowUi {
            run_id: self.run_id.clone(),
            event: value,
        });
    }

    fn record_budget_snapshot(&self, snapshot: BudgetSnapshot) {
        let changed = if let Ok(mut last) = self.last_budget_event.lock() {
            if last.as_ref() == Some(&snapshot) {
                false
            } else {
                *last = Some(snapshot);
                true
            }
        } else {
            false
        };
        if changed {
            self.record_run_event(WorkflowUiEvent::new(budget_event_kind(snapshot)));
        }
    }

    fn record_task_started(
        &self,
        agent_id: &str,
        request: &TaskRequest,
        metadata: &WorkflowTaskSpawnMetadata,
        result: &crate::tools::subagent::SubAgentResult,
    ) {
        // Prefer typed spawn metadata over request fields so panel/history never
        // need to re-derive labels from the child prompt (#4119).
        let label = metadata
            .workflow_task_label
            .clone()
            .or_else(|| request.label.clone());
        self.record_run_event(WorkflowUiEvent::new(WorkflowUiEventKind::TaskStarted(
            Box::new(WorkflowTaskStartedEvent {
                task_id: agent_id.to_string(),
                label,
                role: request.role.clone(),
                profile: request.profile.clone(),
                model: request.model.clone(),
                strength: request.model_strength.clone(),
                thinking: request.thinking.clone(),
                // Prefer spawn metadata (fleet-resolved); fall back to request.
                resolved_role: metadata
                    .resolved_role
                    .clone()
                    .or_else(|| request.role.clone()),
                resolved_profile: metadata
                    .resolved_profile
                    .clone()
                    .or_else(|| request.profile.clone()),
                resolved_provider: metadata.resolved_provider.clone(),
                resolved_model: metadata.resolved_model.clone(),
                route_source: metadata.route_source.clone(),
                worktree: request.worktree,
                workspace: result.workspace.clone(),
                git_branch: result.git_branch.clone(),
                parent_task_id: metadata.parent_task_id.clone(),
                depth: metadata.depth,
                workflow_run_id: metadata.workflow_run_id.clone(),
                workflow_phase_id: metadata.workflow_phase_id.clone(),
                workflow_task_label: metadata.workflow_task_label.clone(),
                workflow_child_index: metadata.workflow_child_index,
            }),
        )));
    }

    fn record_task_request(&self, agent_id: &str, request: &TaskRequest) {
        if let Ok(mut records) = self.task_records.lock() {
            records.insert(
                agent_id.to_string(),
                RuntimeTaskRecord {
                    agent_id: agent_id.to_string(),
                    label: request.label.clone(),
                    status: IrWorkflowRunStatus::Running,
                    output: None,
                    schema_error: None,
                },
            );
        }
        let pending_completion = self
            .completion_state
            .lock()
            .ok()
            .and_then(|state| state.pending.get(agent_id).cloned());
        if let Some(completion) = pending_completion {
            self.record_task_completion(agent_id, &completion);
        }
    }

    fn record_task_completion(&self, agent_id: &str, completion: &TaskCompletion) {
        let mut terminal_event = None;
        if let Ok(mut records) = self.task_records.lock()
            && let Some(record) = records.get_mut(agent_id)
        {
            let was_running = record.status == IrWorkflowRunStatus::Running;
            let (status, output) = task_completion_status(completion);
            record.status = status;
            record.output = output;
            if was_running {
                terminal_event = Some(WorkflowUiEvent::new(WorkflowUiEventKind::TaskCompleted {
                    task_id: agent_id.to_string(),
                    status,
                }));
            }
        }
        if let Some(event) = terminal_event {
            self.record_run_event(event);
        }
    }

    fn record_schema_validation_failure(&self, agent_id: &str, message: String) {
        if let Ok(mut records) = self.task_records.lock()
            && let Some(record) = records.get_mut(agent_id)
        {
            record.status = IrWorkflowRunStatus::Failed;
            record.schema_error = Some(message.clone());
            record.output = Some(message);
        }
    }

    fn task_records_snapshot(&self) -> Vec<RuntimeTaskRecord> {
        self.task_records
            .lock()
            .map(|records| records.values().cloned().collect())
            .unwrap_or_default()
    }

    fn add_waiter_or_complete(&self, agent_id: String, waiter: oneshot::Sender<TaskCompletion>) {
        let mut state = self
            .completion_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(completion) = state.pending.remove(&agent_id) {
            let _ = waiter.send(completion);
        } else {
            state.waiters.insert(agent_id, waiter);
        }
    }

    fn deliver_completion(&self, agent_id: String, completion: TaskCompletion) {
        self.record_task_completion(&agent_id, &completion);
        if let Ok(mut permits) = self.spawn_permits.lock() {
            permits.remove(&agent_id);
        }
        let mut state = self
            .completion_state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some(waiter) = state.waiters.remove(&agent_id) {
            let _ = waiter.send(completion);
        } else {
            state.pending.insert(agent_id, completion);
        }
    }
}

#[derive(Default)]
struct CompletionState {
    waiters: HashMap<String, oneshot::Sender<TaskCompletion>>,
    pending: HashMap<String, TaskCompletion>,
}

#[async_trait]
impl WorkflowDriver for SubAgentWorkflowDriver {
    async fn spawn_task(&self, request: TaskRequest) -> Result<SpawnedTask, DriverError> {
        // Wait for a concurrent slot (max 16 live children per run).
        let permit = self
            .concurrent_gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DriverError::Rejected("workflow concurrent admission closed".into()))?;
        let runtime = self
            .runtime
            .clone()
            .with_parent_completion_tx(self.completion_tx.clone());
        let request_record = request.clone();
        let workflow_child_index = self.child_counter.fetch_add(1, Ordering::SeqCst);
        let workflow_phase_id = request
            .phase
            .as_ref()
            .map(|phase| phase.trim())
            .filter(|phase| !phase.is_empty())
            .map(str::to_string)
            .or_else(|| {
                self.current_phase
                    .lock()
                    .ok()
                    .and_then(|phase| phase.clone())
            });
        let workflow_task_label = request
            .label
            .as_ref()
            .map(|label| label.trim())
            .filter(|label| !label.is_empty())
            .map(str::to_string);
        let identity = WorkflowTaskSpawnIdentity {
            workflow_run_id: self.run_id.clone(),
            workflow_phase_id,
            workflow_task_label,
            workflow_child_index,
        };
        let result =
            match spawn_workflow_task(request, self.manager.clone(), runtime, identity).await {
                Ok(result) => result,
                Err(err) => {
                    drop(permit);
                    return Err(DriverError::Rejected(err.to_string()));
                }
            };
        let task_id = result.result.agent_id.clone();
        if let Ok(mut permits) = self.spawn_permits.lock() {
            permits.insert(task_id.clone(), permit);
        }
        self.record_child(&task_id);
        self.record_task_started(&task_id, &request_record, &result.metadata, &result.result);
        self.record_task_request(&task_id, &request_record);
        if let Some(limit) = self.total_budget {
            let mut manager = self.manager.write().await;
            manager.attach_shared_budget_scope(&task_id, &self.run_id, limit);
        }
        let (tx, rx) = oneshot::channel();
        self.add_waiter_or_complete(task_id.clone(), tx);
        Ok(SpawnedTask {
            task_id,
            completion: rx,
        })
    }

    fn cancel_all(&self) {
        self.force_cancel_all();
    }

    fn budget(&self) -> BudgetSnapshot {
        let snapshot = self.current_budget_snapshot();
        self.record_budget_snapshot(snapshot);
        snapshot
    }

    fn progress(&self, event: ProgressEvent) {
        let mut schema_error = None;
        let (message, ui_event) = match event {
            ProgressEvent::Log { message } => (
                format!("log: {message}"),
                WorkflowUiEvent::new(WorkflowUiEventKind::Log { message }),
            ),
            ProgressEvent::Phase { title } => {
                if let Ok(mut current) = self.current_phase.lock() {
                    *current = Some(title.clone());
                }
                (
                    format!("phase: {title}"),
                    WorkflowUiEvent::new(WorkflowUiEventKind::PhaseStarted { title }),
                )
            }
            ProgressEvent::TaskSchemaValidationFailed { task_id, message } => {
                self.record_schema_validation_failure(&task_id, message.clone());
                schema_error = Some(WorkflowSchemaError {
                    task_id: task_id.clone(),
                    message: message.clone(),
                });
                (
                    format!("schema validation failed for {task_id}: {message}"),
                    WorkflowUiEvent::new(WorkflowUiEventKind::TaskSchemaValidationFailed {
                        task_id,
                        message,
                    }),
                )
            }
        };
        if let Ok(mut runs) = self.state.runs.lock()
            && let Some(record) = runs.get_mut(&self.run_id)
        {
            record.progress.push(message.clone());
            record.events.push(ui_event.clone());
            if let Some(schema_error) = schema_error {
                record.schema_errors.push(schema_error);
            }
        }
        self.state.record_progress(&self.run_id, &message);
        self.state.record_event(&self.run_id, &ui_event);
        // #4122: phase/schema/log progress streams into the live panel path.
        self.emit_ui_event(&ui_event);
    }
}

fn budget_event_kind(snapshot: BudgetSnapshot) -> WorkflowUiEventKind {
    WorkflowUiEventKind::BudgetUpdated {
        total: snapshot.total,
        spent: snapshot.spent,
        remaining: snapshot.remaining(),
    }
}

fn task_completion_status(completion: &TaskCompletion) -> (IrWorkflowRunStatus, Option<String>) {
    match completion {
        TaskCompletion::Completed { text } => (IrWorkflowRunStatus::Succeeded, Some(text.clone())),
        TaskCompletion::Failed { message } => (IrWorkflowRunStatus::Failed, Some(message.clone())),
        TaskCompletion::Cancelled => (IrWorkflowRunStatus::Cancelled, None),
        TaskCompletion::BudgetExhausted { message } => {
            (IrWorkflowRunStatus::BudgetExceeded, Some(message.clone()))
        }
    }
}

fn execution_from_declarative_spec(
    spec: &WorkflowSpec,
    records: Vec<RuntimeTaskRecord>,
    terminal_status: WorkflowRunStatus,
) -> IrWorkflowExecution {
    let by_label = records
        .into_iter()
        .filter_map(|record| record.label.clone().map(|label| (label, record)))
        .collect::<HashMap<_, _>>();
    let mut execution = IrWorkflowExecution::default();
    for node in &spec.nodes {
        push_execution_node(node, &by_label, &mut execution);
    }
    match terminal_status {
        WorkflowRunStatus::Completed => {}
        WorkflowRunStatus::Failed => mark_ir_status(&mut execution, IrWorkflowRunStatus::Failed),
        WorkflowRunStatus::Cancelled => {
            mark_ir_status(&mut execution, IrWorkflowRunStatus::Cancelled);
        }
        WorkflowRunStatus::Running => {
            execution.status = IrWorkflowRunStatus::Running;
        }
    }
    execution
}

fn push_execution_node(
    node: &WorkflowNode,
    records: &HashMap<String, RuntimeTaskRecord>,
    execution: &mut IrWorkflowExecution,
) {
    match node {
        WorkflowNode::Leaf(spec) => push_leaf_execution(spec, records, execution),
        WorkflowNode::BranchSet(spec) => push_branch_execution(spec, records, execution),
        WorkflowNode::Sequence(spec) => push_sequence_execution(spec, records, execution),
        WorkflowNode::Reduce(spec) => push_control_execution(
            spec.id.as_str(),
            ControlNodeKind::Reduce,
            records.get(&spec.id),
            spec.inputs.clone(),
            Some(spec.prompt.clone()),
            execution,
        ),
        WorkflowNode::TeacherReview(spec) => push_control_execution(
            spec.id.as_str(),
            ControlNodeKind::TeacherReview,
            records.get(&spec.id),
            spec.candidates.clone(),
            Some("teacher review not lowered by the production adapter".to_string()),
            execution,
        ),
        WorkflowNode::LoopUntil(spec) => push_control_execution(
            spec.id.as_str(),
            ControlNodeKind::LoopUntil,
            records.get(&spec.id),
            spec.children.iter().map(declarative_node_id).collect(),
            Some("loop_until not lowered by the production adapter".to_string()),
            execution,
        ),
        WorkflowNode::Cond(spec) => push_control_execution(
            spec.id.as_str(),
            ControlNodeKind::Cond,
            records.get(&spec.id),
            spec.then_nodes
                .iter()
                .chain(spec.else_nodes.iter())
                .map(declarative_node_id)
                .collect(),
            Some("cond not lowered by the production adapter".to_string()),
            execution,
        ),
        WorkflowNode::Expand(spec) => push_control_execution(
            spec.id.as_str(),
            ControlNodeKind::Expand,
            records.get(&spec.id),
            Vec::new(),
            Some(format!("expand not lowered from {}", spec.source)),
            execution,
        ),
    }
}

fn push_leaf_execution(
    spec: &LeafSpec,
    records: &HashMap<String, RuntimeTaskRecord>,
    execution: &mut IrWorkflowExecution,
) {
    let record = records.get(&spec.id);
    let status = record
        .map(|record| record.status)
        .unwrap_or(IrWorkflowRunStatus::Pending);
    mark_ir_status(execution, status);
    execution.leaf_results.push(LeafResult {
        leaf_id: spec.id.clone(),
        task_id: record
            .map(|record| record.agent_id.clone())
            .unwrap_or_else(|| spec.id.clone()),
        role: spec.role.clone(),
        profile: spec.profile.clone(),
        status,
        usage: WorkflowUsage::default(),
        memo_usage: WorkflowMemoUsage::default(),
        output: record.and_then(|record| record.output.clone()),
        artifacts: Vec::new(),
        schema_error: record.and_then(|record| record.schema_error.clone()),
    });
}

fn push_branch_execution(
    spec: &BranchSpec,
    records: &HashMap<String, RuntimeTaskRecord>,
    execution: &mut IrWorkflowExecution,
) {
    let before = execution.leaf_results.len();
    for child in &spec.children {
        push_execution_node(child, records, execution);
    }
    let status = aggregate_ir_status(
        execution.leaf_results[before..]
            .iter()
            .map(|result| result.status),
    );
    mark_ir_status(execution, status);
    execution.branch_results.push(BranchResult {
        branch_id: spec.id.clone(),
        task_id: spec.id.clone(),
        status,
        usage: WorkflowUsage::default(),
        memo_usage: WorkflowMemoUsage::default(),
        artifacts: Vec::new(),
        notes: Some("production driver branch receipt from child task outcomes".to_string()),
    });
    execution.control_node_results.push(ControlNodeResult {
        node_id: spec.id.clone(),
        kind: ControlNodeKind::BranchSet,
        status,
        selected_children: spec.children.iter().map(declarative_node_id).collect(),
        summary: Some("branch set lowered into production child tasks".to_string()),
    });
}

fn push_sequence_execution(
    spec: &SequenceSpec,
    records: &HashMap<String, RuntimeTaskRecord>,
    execution: &mut IrWorkflowExecution,
) {
    let before_leaf = execution.leaf_results.len();
    let before_control = execution.control_node_results.len();
    for child in &spec.children {
        push_execution_node(child, records, execution);
    }
    let status = aggregate_ir_status(
        execution.leaf_results[before_leaf..]
            .iter()
            .map(|result| result.status)
            .chain(
                execution.control_node_results[before_control..]
                    .iter()
                    .map(|result| result.status),
            ),
    );
    mark_ir_status(execution, status);
    execution.control_node_results.push(ControlNodeResult {
        node_id: spec.id.clone(),
        kind: ControlNodeKind::Sequence,
        status,
        selected_children: spec.children.iter().map(declarative_node_id).collect(),
        summary: Some("sequence lowered in declaration order".to_string()),
    });
}

fn push_control_execution(
    node_id: &str,
    kind: ControlNodeKind,
    record: Option<&RuntimeTaskRecord>,
    selected_children: Vec<String>,
    fallback_summary: Option<String>,
    execution: &mut IrWorkflowExecution,
) {
    let status = record
        .map(|record| record.status)
        .unwrap_or(IrWorkflowRunStatus::Pending);
    mark_ir_status(execution, status);
    execution.control_node_results.push(ControlNodeResult {
        node_id: node_id.to_string(),
        kind,
        status,
        selected_children,
        summary: record
            .and_then(|record| record.output.clone())
            .or(fallback_summary),
    });
}

fn aggregate_ir_status(
    statuses: impl IntoIterator<Item = IrWorkflowRunStatus>,
) -> IrWorkflowRunStatus {
    let mut saw_pending = false;
    let mut saw_running = false;
    for status in statuses {
        match status {
            IrWorkflowRunStatus::BudgetExceeded => return IrWorkflowRunStatus::BudgetExceeded,
            IrWorkflowRunStatus::Cancelled => return IrWorkflowRunStatus::Cancelled,
            IrWorkflowRunStatus::Failed | IrWorkflowRunStatus::ReplayDiverged => {
                return IrWorkflowRunStatus::Failed;
            }
            IrWorkflowRunStatus::Running => saw_running = true,
            IrWorkflowRunStatus::Pending => saw_pending = true,
            IrWorkflowRunStatus::Succeeded => {}
        }
    }
    if saw_running {
        IrWorkflowRunStatus::Running
    } else if saw_pending {
        IrWorkflowRunStatus::Pending
    } else {
        IrWorkflowRunStatus::Succeeded
    }
}

fn mark_ir_status(execution: &mut IrWorkflowExecution, status: IrWorkflowRunStatus) {
    match status {
        IrWorkflowRunStatus::Failed | IrWorkflowRunStatus::ReplayDiverged => {
            execution.mark_failed()
        }
        IrWorkflowRunStatus::Cancelled => execution.mark_cancelled(),
        IrWorkflowRunStatus::BudgetExceeded => execution.mark_budget_exceeded(),
        IrWorkflowRunStatus::Running => {
            if execution.status == IrWorkflowRunStatus::Succeeded {
                execution.status = IrWorkflowRunStatus::Running;
            }
        }
        IrWorkflowRunStatus::Pending => {
            if execution.status == IrWorkflowRunStatus::Succeeded {
                execution.status = IrWorkflowRunStatus::Pending;
            }
        }
        IrWorkflowRunStatus::Succeeded => {}
    }
}

fn declarative_node_id(node: &WorkflowNode) -> String {
    match node {
        WorkflowNode::BranchSet(spec) => spec.id.clone(),
        WorkflowNode::Leaf(spec) => spec.id.clone(),
        WorkflowNode::Sequence(spec) => spec.id.clone(),
        WorkflowNode::Reduce(spec) => spec.id.clone(),
        WorkflowNode::TeacherReview(spec) => spec.id.clone(),
        WorkflowNode::LoopUntil(spec) => spec.id.clone(),
        WorkflowNode::Cond(spec) => spec.id.clone(),
        WorkflowNode::Expand(spec) => spec.id.clone(),
    }
}

fn spawn_completion_pump(
    driver: Arc<SubAgentWorkflowDriver>,
    mut rx: mpsc::UnboundedReceiver<SubAgentCompletion>,
) {
    spawn_supervised(
        "workflow-completion-pump",
        std::panic::Location::caller(),
        async move {
            while let Some(completion) = rx.recv().await {
                let agent_id = completion.agent_id.clone();
                let task_completion =
                    completion_from_manager(driver.manager.clone(), &agent_id, completion.payload)
                        .await;
                driver.deliver_completion(agent_id, task_completion);
            }
        },
    );
}

async fn completion_from_manager(
    manager: SharedSubAgentManager,
    agent_id: &str,
    fallback_payload: String,
) -> TaskCompletion {
    for _ in 0..50 {
        let snapshot = {
            let manager = manager.read().await;
            manager.get_result(agent_id).ok()
        };
        if let Some(snapshot) = snapshot
            && snapshot.status != SubAgentStatus::Running
        {
            return match snapshot.status {
                SubAgentStatus::Completed => TaskCompletion::Completed {
                    text: snapshot.result.unwrap_or(fallback_payload),
                },
                SubAgentStatus::Failed(message) => TaskCompletion::Failed { message },
                SubAgentStatus::Interrupted(message) => TaskCompletion::Failed { message },
                SubAgentStatus::Cancelled => TaskCompletion::Cancelled,
                SubAgentStatus::BudgetExhausted => TaskCompletion::BudgetExhausted {
                    message: "sub-agent budget exhausted".to_string(),
                },
                SubAgentStatus::Running => unreachable!("guarded above"),
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    TaskCompletion::Failed {
        message: format!("sub-agent '{agent_id}' did not report a terminal status within 1s"),
    }
}

fn cancel_child_agents(manager: SharedSubAgentManager, ids: Vec<String>) {
    if ids.is_empty() {
        return;
    }
    if let Ok(mut manager_guard) = manager.try_write() {
        for id in ids {
            let _ = manager_guard.cancel_agent(&id);
        }
        return;
    }
    if tokio::runtime::Handle::try_current().is_ok() {
        spawn_supervised(
            "workflow-cancel-children",
            std::panic::Location::caller(),
            async move {
                let mut manager_guard = manager.write().await;
                for id in ids {
                    let _ = manager_guard.cancel_agent(&id);
                }
            },
        );
    }
}

fn lock_mutex<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, ToolError> {
    mutex
        .lock()
        .map_err(|_| ToolError::execution_failed("workflow state lock poisoned"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

mod journal {
    use super::{
        SharedWorkflowControllers, SharedWorkflowRuns, WorkflowRunRecord, WorkflowRunStatus,
        WorkflowUiEvent,
    };
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::{BufRead, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, OnceLock};
    use tracing::warn;

    const CODEWHALE_DIR: &str = ".codewhale";
    const WORKFLOW_RUNS_FILE: &str = "workflow-runs.jsonl";

    /// Per-workspace workflow state shared across tool-registry rebuilds.
    pub(super) struct WorkflowWorkspaceState {
        pub runs: SharedWorkflowRuns,
        pub controllers: SharedWorkflowControllers,
        journal: WorkflowRunJournal,
    }

    impl WorkflowWorkspaceState {
        pub fn open(workspace: &Path) -> Arc<Self> {
            let journal = WorkflowRunJournal::open(workspace);
            let runs = Arc::new(Mutex::new(journal.hydrate_runs()));
            Arc::new(Self {
                runs,
                controllers: Arc::new(Mutex::new(HashMap::new())),
                journal,
            })
        }

        pub fn record_snapshot(&self, record: &WorkflowRunRecord) {
            if let Err(err) = self.journal.append_snapshot(record) {
                warn!("workflow journal snapshot failed: {err}");
            }
        }

        pub fn record_progress(&self, run_id: &str, message: &str) {
            if let Err(err) = self.journal.append_progress(run_id, message) {
                warn!("workflow journal progress failed: {err}");
            }
        }

        pub fn record_event(&self, run_id: &str, event: &WorkflowUiEvent) {
            if let Err(err) = self.journal.append_event(run_id, event) {
                warn!("workflow journal event failed: {err}");
            }
        }
    }

    fn workspace_store() -> &'static Mutex<HashMap<PathBuf, Arc<WorkflowWorkspaceState>>> {
        static STORE: OnceLock<Mutex<HashMap<PathBuf, Arc<WorkflowWorkspaceState>>>> =
            OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(super) fn shared_workflow_state(workspace: &Path) -> Arc<WorkflowWorkspaceState> {
        let key = workspace
            .canonicalize()
            .unwrap_or_else(|_| workspace.to_path_buf());
        let mut store = workspace_store()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        store
            .entry(key)
            .or_insert_with(|| WorkflowWorkspaceState::open(workspace))
            .clone()
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "snake_case")]
    enum WorkflowJournalRecord {
        // Boxed: a full run record dwarfs the progress variant
        // (clippy::large_enum_variant).
        Snapshot {
            run: Box<WorkflowRunRecord>,
        },
        Progress {
            run_id: String,
            message: String,
        },
        Event {
            run_id: String,
            event: Box<WorkflowUiEvent>,
        },
    }

    #[derive(Debug)]
    struct WorkflowRunJournal {
        ledger_path: PathBuf,
    }

    impl WorkflowRunJournal {
        fn open(workspace: &Path) -> Self {
            let dir = workspace.join(CODEWHALE_DIR);
            if let Err(err) = std::fs::create_dir_all(&dir) {
                warn!(
                    "workflow journal dir create failed ({}): {err}",
                    dir.display()
                );
            }
            let ledger_path = dir.join(WORKFLOW_RUNS_FILE);
            if !ledger_path.exists()
                && let Err(err) = std::fs::write(&ledger_path, "")
            {
                warn!(
                    "workflow journal create failed ({}): {err}",
                    ledger_path.display()
                );
            }
            Self { ledger_path }
        }

        fn hydrate_runs(&self) -> HashMap<String, WorkflowRunRecord> {
            let file = match std::fs::File::open(&self.ledger_path) {
                Ok(file) => file,
                Err(_) => return HashMap::new(),
            };
            let mut runs = HashMap::new();
            for line in std::io::BufReader::new(file).lines() {
                let Ok(line) = line else { continue };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let record = match serde_json::from_str::<WorkflowJournalRecord>(trimmed) {
                    Ok(record) => record,
                    Err(err) => {
                        warn!("workflow journal skipped malformed line: {err}");
                        continue;
                    }
                };
                match record {
                    WorkflowJournalRecord::Snapshot { run } => {
                        runs.insert(run.run_id.clone(), *run);
                    }
                    WorkflowJournalRecord::Progress { run_id, message } => {
                        if let Some(run) = runs.get_mut(&run_id) {
                            run.progress.push(message);
                        }
                    }
                    WorkflowJournalRecord::Event { run_id, event } => {
                        if let Some(run) = runs.get_mut(&run_id) {
                            run.events.push(*event);
                        }
                    }
                }
            }
            // A run journaled as Running belongs to a process that is gone;
            // without this it would show as live forever after a restart.
            for run in runs.values_mut() {
                if run.status == WorkflowRunStatus::Running {
                    run.status = WorkflowRunStatus::Failed;
                    run.error = Some(
                        "process exited before the run completed (recovered on startup)"
                            .to_string(),
                    );
                }
            }
            runs
        }

        fn append_record(&self, record: &WorkflowJournalRecord) -> std::io::Result<()> {
            let mut line = serde_json::to_string(record)
                .map_err(|err| std::io::Error::other(err.to_string()))?;
            line.push('\n');
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.ledger_path)?;
            file.write_all(line.as_bytes())?;
            file.flush()?;
            Ok(())
        }

        fn append_snapshot(&self, record: &WorkflowRunRecord) -> std::io::Result<()> {
            self.append_record(&WorkflowJournalRecord::Snapshot {
                run: Box::new(record.clone()),
            })
        }

        fn append_progress(&self, run_id: &str, message: &str) -> std::io::Result<()> {
            self.append_record(&WorkflowJournalRecord::Progress {
                run_id: run_id.to_string(),
                message: message.to_string(),
            })
        }

        fn append_event(&self, run_id: &str, event: &WorkflowUiEvent) -> std::io::Result<()> {
            self.append_record(&WorkflowJournalRecord::Event {
                run_id: run_id.to_string(),
                event: Box::new(event.clone()),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use super::super::WorkflowUiEventKind;
        use super::*;

        fn sample_record(run_id: &str, status: WorkflowRunStatus) -> WorkflowRunRecord {
            WorkflowRunRecord {
                run_id: run_id.to_string(),
                status,
                started_at_ms: 1,
                completed_at_ms: None,
                source_path: None,
                workflow_id: Some("fixture".to_string()),
                workflow_goal: Some("journal test".to_string()),
                token_budget: None,
                child_ids: Vec::new(),
                progress: Vec::new(),
                events: Vec::new(),
                schema_errors: Vec::new(),
                result: None,
                execution: None,
                error: None,
                verify_on_complete: false,
                verification: None,
                plan_approval: None,
            }
        }

        #[test]
        fn workflow_journal_hydrates_snapshots_and_progress() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let state = WorkflowWorkspaceState::open(tmp.path());
            let running = sample_record("workflow_abc", WorkflowRunStatus::Running);
            state.record_snapshot(&running);
            state.record_progress("workflow_abc", "phase: scan");
            state.record_event(
                "workflow_abc",
                &WorkflowUiEvent::at(
                    5,
                    WorkflowUiEventKind::PhaseStarted {
                        title: "scan".to_string(),
                    },
                ),
            );

            let completed = WorkflowRunRecord {
                status: WorkflowRunStatus::Completed,
                completed_at_ms: Some(99),
                progress: vec!["phase: scan".to_string()],
                events: vec![WorkflowUiEvent::at(
                    5,
                    WorkflowUiEventKind::PhaseStarted {
                        title: "scan".to_string(),
                    },
                )],
                ..sample_record("workflow_abc", WorkflowRunStatus::Completed)
            };
            state.record_snapshot(&completed);

            let reloaded = WorkflowWorkspaceState::open(tmp.path());
            let runs = reloaded
                .runs
                .lock()
                .expect("runs lock")
                .get("workflow_abc")
                .cloned()
                .expect("hydrated run");
            assert_eq!(runs.status, WorkflowRunStatus::Completed);
            assert_eq!(runs.progress, vec!["phase: scan"]);
            assert_eq!(runs.events.len(), 1);
            assert_eq!(runs.events[0].event_type(), "phase_started");
            assert_eq!(runs.completed_at_ms, Some(99));
        }

        #[test]
        fn workflow_journal_marks_orphaned_running_runs_failed() {
            let tmp = tempfile::tempdir().expect("tempdir");
            let state = WorkflowWorkspaceState::open(tmp.path());
            state.record_snapshot(&sample_record(
                "workflow_orphan",
                WorkflowRunStatus::Running,
            ));

            let reloaded = WorkflowWorkspaceState::open(tmp.path());
            let run = reloaded
                .runs
                .lock()
                .expect("runs lock")
                .get("workflow_orphan")
                .cloned()
                .expect("hydrated run");
            assert_eq!(run.status, WorkflowRunStatus::Failed);
            assert!(
                run.error
                    .as_deref()
                    .is_some_and(|error| error.contains("process exited")),
                "expected orphan recovery error, got {:?}",
                run.error
            );
        }
    }
}

use journal::{WorkflowWorkspaceState, shared_workflow_state};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::DeepSeekClient;
    use crate::tools::ToolRegistryBuilder;
    use crate::tools::subagent::{SubAgentRuntime, new_shared_subagent_manager};
    use axum::{Json, Router, routing::post};
    use codewhale_workflow::{IsolationMode, leaf_is_write_capable};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn declarative_detection_matches_indented_and_nonleading_workflow_calls() {
        // column-0 forms
        assert!(looks_like_declarative_workflow("workflow({ tasks: [] })"));
        assert!(looks_like_declarative_workflow(
            "export default workflow({})"
        ));
        // #dogfood 0.8.67: a leading statement/comment followed by an INDENTED
        // top-level workflow( call must still be detected as declarative.
        assert!(looks_like_declarative_workflow(
            "// build the run\n  workflow({\n    tasks: [],\n  })"
        ));
        // imperative scripts must not be misdetected as declarative
        assert!(!looks_like_declarative_workflow(
            "return await parallel([() => task({ description: \"x\" })]);"
        ));
        assert!(!looks_like_declarative_workflow("const x = myworkflow(1);"));
    }

    #[test]
    fn workflow_action_defaults_to_start() {
        assert_eq!(
            parse_workflow_action(&json!({})).unwrap(),
            WorkflowAction::Start
        );
        assert_eq!(
            parse_workflow_action(&json!({"action": "run"})).unwrap(),
            WorkflowAction::Run
        );
    }

    #[test]
    fn parallel_write_children_default_to_worktree_isolation() {
        // #4120: write-capable parallel leaves get worktree: true by default.
        let source = r#"
export default workflow({
  "goal": "parallel write isolation default",
  "nodes": [
    {
      "branch": {
        "id": "implement",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "left",
              "prompt": "Patch left lane",
              "agent_type": "implementer",
              "mode": "read_write",
              "file_scope": ["src/left.rs"]
            }
          },
          {
            "agent": {
              "id": "right",
              "prompt": "Patch right lane",
              "agent_type": "implementer",
              "mode": "read_write",
              "file_scope": ["src/right.rs"]
            }
          }
        ]
      }
    }
  ]
});
"#;
        let adapted = adapt_workflow_source(source, None).expect("lower parallel write workflow");
        let spec = adapted.spec.expect("declarative spec");
        let WorkflowNode::BranchSet(branch) = &spec.nodes[0] else {
            panic!("expected branch_set");
        };
        assert!(branch.parallel);
        for child in &branch.children {
            let WorkflowNode::Leaf(leaf) = child else {
                panic!("expected leaf");
            };
            assert!(leaf_is_write_capable(leaf));
            assert!(
                leaf_wants_worktree(leaf, true),
                "parallel write leaf {} should default to worktree",
                leaf.id
            );
            assert_eq!(leaf.isolation, IsolationMode::Auto);
        }
        assert!(
            adapted.source.contains("worktree: true"),
            "lowered JS should request worktree isolation:\n{}",
            adapted.source
        );
        // Both parallel children should carry the worktree flag.
        assert_eq!(
            adapted.source.matches("worktree: true").count(),
            2,
            "each parallel write child should get worktree: true:\n{}",
            adapted.source
        );
    }

    #[test]
    fn parallel_write_same_worktree_requires_explicit_shared_isolation() {
        // #4120: isolation: shared is the approved same-worktree override.
        let source = r#"
export default workflow({
  "goal": "parallel write same-worktree override",
  "nodes": [
    {
      "branch": {
        "id": "implement",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "shared-writer",
              "prompt": "Patch in the parent checkout",
              "agent_type": "implementer",
              "mode": "read_write",
              "isolation": "shared",
              "file_scope": ["src/shared.rs"]
            }
          },
          {
            "agent": {
              "id": "isolated-writer",
              "prompt": "Patch in a worktree",
              "agent_type": "implementer",
              "mode": "read_write",
              "isolation": "worktree",
              "file_scope": ["src/isolated.rs"]
            }
          }
        ]
      }
    }
  ]
});
"#;
        let adapted =
            adapt_workflow_source(source, None).expect("lower same-worktree override workflow");
        let spec = adapted.spec.expect("declarative spec");
        let WorkflowNode::BranchSet(branch) = &spec.nodes[0] else {
            panic!("expected branch_set");
        };
        let leaves: Vec<&LeafSpec> = branch
            .children
            .iter()
            .map(|child| match child {
                WorkflowNode::Leaf(leaf) => leaf,
                _ => panic!("expected leaf"),
            })
            .collect();
        assert_eq!(leaves[0].isolation, IsolationMode::Shared);
        assert!(
            !leaf_wants_worktree(leaves[0], true),
            "explicit shared should keep same-worktree"
        );
        assert_eq!(leaves[1].isolation, IsolationMode::Worktree);
        assert!(leaf_wants_worktree(leaves[1], true));

        // Only the explicit worktree child should emit worktree: true.
        assert_eq!(
            adapted.source.matches("worktree: true").count(),
            1,
            "same-worktree override must not force worktree on shared leaf:\n{}",
            adapted.source
        );
        assert!(
            adapted.source.contains("shared-writer") && adapted.source.contains("isolated-writer"),
            "both children should still be lowered:\n{}",
            adapted.source
        );
    }

    #[test]
    fn parallel_read_only_children_do_not_default_to_worktree() {
        let source = r#"
export default workflow({
  "goal": "parallel read-only stays shared",
  "nodes": [
    {
      "branch": {
        "id": "audit",
        "parallel": true,
        "children": [
          {
            "agent": {
              "id": "review-a",
              "prompt": "Review A",
              "agent_type": "review",
              "mode": "read_only"
            }
          },
          {
            "agent": {
              "id": "review-b",
              "prompt": "Review B",
              "agent_type": "verifier",
              "mode": "read_only"
            }
          }
        ]
      }
    }
  ]
});
"#;
        let adapted = adapt_workflow_source(source, None).expect("lower parallel read-only");
        assert!(
            !adapted.source.contains("worktree: true"),
            "read-only parallel children should not get worktree isolation:\n{}",
            adapted.source
        );
    }

    #[test]
    fn sequential_write_children_do_not_default_to_worktree() {
        let source = r#"
export default workflow({
  "goal": "sequential write stays shared by default",
  "nodes": [
    {
      "sequence": {
        "id": "implement",
        "children": [
          {
            "agent": {
              "id": "writer",
              "prompt": "Patch sequentially",
              "agent_type": "implementer",
              "mode": "read_write",
              "file_scope": ["src/main.rs"]
            }
          }
        ]
      }
    }
  ]
});
"#;
        let adapted = adapt_workflow_source(source, None).expect("lower sequential write");
        assert!(
            !adapted.source.contains("worktree: true"),
            "sequential writes should not default to worktree:\n{}",
            adapted.source
        );
    }

    #[test]
    fn inline_script_and_source_path_are_mutually_exclusive() {
        let ctx = ToolContext::new(".");
        let err = workflow_source(
            &json!({
                "script": "return 1;",
                "source_path": "workflow.js"
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("exactly one of script, source_path, or plan"),
            "{err}"
        );
    }

    #[test]
    fn structured_plan_lowers_to_parallel_not_promise_all() {
        // #4124: planner plan → JS with parallel() partial-success semantics.
        let ctx = ToolContext::new(".");
        let source = workflow_source(
            &json!({
                "plan": {
                    "goal": "audit two independent scopes",
                    "risk": "read_only",
                    "max_children": 8,
                    "token_budget": 120000,
                    "phases": [{
                        "id": "scout",
                        "title": "Scout",
                        "children": [
                            {
                                "id": "left",
                                "label": "left-lane",
                                "prompt": "Inspect crates/left",
                                "type": "explore"
                            },
                            {
                                "id": "right",
                                "prompt": "Inspect crates/right",
                                "type": "explore"
                            }
                        ]
                    }]
                }
            }),
            &ctx,
        )
        .expect("structured plan should lower");

        assert!(
            source.source.contains("await parallel(["),
            "lowered JS must use parallel():\n{}",
            source.source
        );
        assert!(
            !source.source.contains("Promise.all"),
            "lowered JS must not use raw Promise.all:\n{}",
            source.source
        );
        assert!(
            source.source.contains("() => task("),
            "parallel slots should be thunks:\n{}",
            source.source
        );
        let spec = source.spec.expect("plan should produce WorkflowSpec");
        assert_eq!(spec.goal, "audit two independent scopes");
        assert_eq!(spec.budget.max_tokens, Some(120000));
        assert_eq!(spec.nodes.len(), 1);
        let WorkflowNode::BranchSet(branch) = &spec.nodes[0] else {
            panic!("expected parallel branch for multi-child phase");
        };
        assert!(branch.parallel);
        assert_eq!(branch.children.len(), 2);
    }

    #[test]
    fn structured_plan_validation_errors_are_typed() {
        let ctx = ToolContext::new(".");
        let missing_goal = workflow_source(
            &json!({
                "plan": {
                    "goal": "   ",
                    "children": [{ "prompt": "do work" }]
                }
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(missing_goal.to_string().contains("goal"), "{missing_goal}");

        let over_limit = workflow_source(
            &json!({
                "plan": {
                    "goal": "too many children",
                    "max_children": 1,
                    "children": [
                        { "id": "a", "prompt": "one" },
                        { "id": "b", "prompt": "two" }
                    ]
                }
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(
            over_limit.to_string().contains("max_children"),
            "{over_limit}"
        );

        let bad_type = workflow_source(
            &json!({
                "plan": {
                    "goal": "bad type",
                    "children": [{ "prompt": "x", "type": "wizard" }]
                }
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(
            bad_type.to_string().contains("Invalid plan child type"),
            "{bad_type}"
        );

        let exclusive = workflow_source(
            &json!({
                "script": "return 1;",
                "plan": { "goal": "x", "children": [{ "prompt": "y" }] }
            }),
            &ctx,
        )
        .unwrap_err();
        assert!(
            exclusive
                .to_string()
                .contains("exactly one of script, source_path, or plan"),
            "{exclusive}"
        );
    }

    #[test]
    fn declarative_parallel_branch_uses_parallel_helper() {
        let source = r#"
export default workflow({
  "goal": "partial success fan-out",
  "nodes": [
    {
      "branch": {
        "id": "fan",
        "parallel": true,
        "children": [
          { "agent": { "id": "a", "prompt": "A", "agent_type": "explore", "mode": "read_only" } },
          { "agent": { "id": "b", "prompt": "B", "agent_type": "explore", "mode": "read_only" } }
        ]
      }
    }
  ]
});
"#;
        let adapted = adapt_workflow_source(source, None).expect("lower declarative");
        assert!(
            adapted.source.contains("await parallel(["),
            "declarative parallel must lower via parallel():\n{}",
            adapted.source
        );
        assert!(
            !adapted.source.contains("Promise.all"),
            "must not emit raw Promise.all:\n{}",
            adapted.source
        );
    }

    #[test]
    fn source_path_must_stay_inside_workspace_without_trust_mode() {
        let workspace = tempfile::tempdir().expect("workspace tempdir");
        let outside = tempfile::tempdir().expect("outside tempdir");
        let outside_path = outside.path().join("outside.workflow.js");
        std::fs::write(&outside_path, "return 1;").expect("outside workflow source");
        let ctx = ToolContext::new(workspace.path().to_path_buf());

        let err = workflow_source(
            &json!({
                "source_path": outside_path
            }),
            &ctx,
        )
        .expect_err("outside source_path should be denied");

        assert!(
            err.to_string().contains("must stay inside the workspace"),
            "{err}"
        );
    }

    #[test]
    fn subagent_tool_surface_registers_workflow_and_agent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let runtime = SubAgentRuntime::new(
            stub_client(),
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let registry = ToolRegistryBuilder::new()
            .with_subagent_tools(manager, runtime)
            .build(ctx);

        assert!(registry.contains("workflow"));
        assert!(registry.contains("agent"));
        assert!(
            registry
                .to_api_tools()
                .iter()
                .any(|tool| tool.name == "workflow")
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_run_dispatches_task_through_subagent_manager() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, calls) = fake_chat_client("child done").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "script": "phase('dispatch'); log('starting child'); const out = await task({ description: 'say done', type: 'explore', allowedTools: [], label: 'inspect-child', model: 'deepseek-v4-flash', modelStrength: 'same', thinking: 'low' }); return { out };"
                }),
                &ctx,
            )
            .await
            .expect("workflow run should complete");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(payload["status"], "completed", "{payload}");
        assert_eq!(payload["result"]["out"], "child done");
        assert_eq!(payload["child_ids"].as_array().unwrap().len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let child_id = payload["child_ids"][0].as_str().unwrap();
        let events = payload["events"].as_array().expect("events array");
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "phase_started" && event["title"] == "dispatch"),
            "{events:#?}"
        );
        assert!(
            events
                .iter()
                .any(|event| event["type"] == "log" && event["message"] == "starting child"),
            "{events:#?}"
        );
        assert!(
            events.iter().any(|event| event["type"] == "budget_updated"),
            "{events:#?}"
        );
        let task_started = events
            .iter()
            .find(|event| event["type"] == "task_started")
            .expect("task_started event");
        assert_eq!(task_started["task_id"], child_id);
        assert_eq!(task_started["label"], "inspect-child");
        assert!(task_started["profile"].is_null());
        assert_eq!(task_started["model"], "deepseek-v4-flash");
        assert_eq!(task_started["strength"], "same");
        assert_eq!(task_started["thinking"], "low");
        assert_eq!(task_started["resolved_provider"], "deepseek");
        assert_eq!(task_started["resolved_model"], "deepseek-v4-flash");
        assert!(
            task_started["route_source"]
                .as_str()
                .is_some_and(|source| source.contains("explicit model id")),
            "{}",
            task_started["route_source"]
        );
        assert_eq!(task_started["worktree"], false);
        assert!(task_started["parent_task_id"].is_null());
        assert_eq!(task_started["depth"], 1);
        // #4119: workflow identity on spawn / task_started metadata.
        assert_eq!(
            task_started["workflow_run_id"].as_str(),
            payload["run_id"].as_str()
        );
        assert_eq!(task_started["workflow_phase_id"], "dispatch");
        assert_eq!(task_started["workflow_task_label"], "inspect-child");
        assert_eq!(task_started["workflow_child_index"], 0);
        assert!(
            events.iter().any(|event| event["type"] == "task_completed"
                && event["task_id"] == child_id
                && event["status"] == "succeeded"),
            "{events:#?}"
        );
        let child = manager
            .read()
            .await
            .get_result(child_id)
            .expect("child result");
        assert_eq!(child.status, SubAgentStatus::Completed);
        assert_eq!(child.result.as_deref(), Some("child done"));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_spawn_records_carry_child_index_and_phase_metadata() {
        // #4119: sequential children get monotonic workflow_child_index and
        // inherit the active phase when task options omit `phase`.
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let (client, calls) = fake_chat_client("ok").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "script": "phase('alpha'); await task({ description: 'first', type: 'explore', allowedTools: [], label: 'one' }); phase('beta'); await task({ description: 'second', type: 'explore', allowedTools: [], label: 'two', phase: 'beta-explicit' }); return { ok: true };"
                }),
                &ctx,
            )
            .await
            .expect("workflow run should complete");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");
        assert_eq!(payload["status"], "completed", "{payload}");
        assert_eq!(payload["child_ids"].as_array().unwrap().len(), 2);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let mut started: Vec<&Value> = payload["events"]
            .as_array()
            .expect("events")
            .iter()
            .filter(|event| event["type"] == "task_started")
            .collect();
        started.sort_by_key(|event| event["workflow_child_index"].as_u64().unwrap_or(u64::MAX));
        assert_eq!(started.len(), 2, "{started:#?}");

        assert_eq!(started[0]["workflow_run_id"], payload["run_id"]);
        assert_eq!(started[0]["workflow_phase_id"], "alpha");
        assert_eq!(started[0]["workflow_task_label"], "one");
        assert_eq!(started[0]["workflow_child_index"], 0);
        assert_eq!(started[0]["label"], "one");

        assert_eq!(started[1]["workflow_run_id"], payload["run_id"]);
        // Explicit task phase wins over the driver's current phase.
        assert_eq!(started[1]["workflow_phase_id"], "beta-explicit");
        assert_eq!(started[1]["workflow_task_label"], "two");
        assert_eq!(started[1]["workflow_child_index"], 1);
        assert_eq!(started[1]["label"], "two");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn declarative_parallel_spawn_failure_nulls_slot_and_continues() {
        // #4124: parallel() is all-settled — a rejected spawn becomes a null slot
        // (with a breadcrumb) instead of aborting the rest of the script the way
        // raw Promise.all would. Downstream reduce still runs on partial results.
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, calls) = fake_chat_client("reduce-with-partial").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager,
        );
        let tool = WorkflowTool::new(runtime.manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "script": r#"export default workflow({
                        "goal": "partial success fan-out",
                        "nodes": [
                            {
                                "branch": {
                                    "id": "parallel",
                                    "parallel": true,
                                    "children": [
                                        {
                                            "agent": {
                                                "id": "bad-profile",
                                                "prompt": "This child should be rejected before model execution.",
                                                "profile": "missing-profile"
                                            }
                                        }
                                    ]
                                }
                            },
                            {
                                "reduce": {
                                    "id": "summary",
                                    "inputs": ["bad-profile"],
                                    "prompt": "Summarize whatever survived the parallel fan-out."
                                }
                            }
                        ]
                    });"#
                }),
                &ctx,
            )
            .await
            .expect("partial-success workflow still returns run record");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(payload["status"], "completed");
        assert!(payload["error"].is_null());
        assert!(
            payload["result"]["bad-profile"].is_null(),
            "failed parallel slot should be null: {}",
            payload["result"]
        );
        assert_eq!(payload["result"]["summary"], "reduce-with-partial");
        let progress = payload["progress"]
            .as_array()
            .expect("progress array")
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            progress.contains("missing-profile") && progress.contains("dropped a failed slot"),
            "breadcrumb should surface the spawn rejection:\n{progress}"
        );
        assert!(
            calls.load(Ordering::SeqCst) >= 1,
            "reduce should still run after a null parallel slot"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn declarative_dependency_results_are_forwarded_to_downstream_prompt() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, calls, bodies) = fake_chat_client_capturing("upstream-output").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager,
        );
        let tool = WorkflowTool::new(runtime.manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "script": r#"export default workflow({
                        "goal": "dependency forwarding",
                        "nodes": [
                            {
                                "agent": {
                                    "id": "first",
                                    "prompt": "Produce the upstream finding.",
                                    "agent_type": "review"
                                }
                            },
                            {
                                "agent": {
                                    "id": "second",
                                    "prompt": "Use the upstream finding.",
                                    "agent_type": "review",
                                    "depends_on_results": ["first"]
                                }
                            }
                        ]
                    });"#
                }),
                &ctx,
            )
            .await
            .expect("dependency workflow should complete");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(payload["status"], "completed", "{payload}");
        assert_eq!(payload["execution"]["status"], "succeeded");
        assert_eq!(
            payload["execution"]["leaf_results"][0]["output"],
            "upstream-output"
        );
        assert_eq!(
            payload["execution"]["leaf_results"][1]["output"],
            "upstream-output"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let bodies = bodies.lock().expect("captured bodies");
        let second_body = bodies.get(1).expect("second provider call").to_string();
        assert!(second_body.contains("--- first ---"), "{second_body}");
        assert!(second_body.contains("upstream-output"), "{second_body}");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_status_lists_compact_typed_receipts() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, _calls) = fake_chat_client("status-output").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager,
        );
        let tool = WorkflowTool::new(runtime.manager.clone(), runtime);

        let run = tool
            .execute(
                json!({
                    "action": "run",
                    "script": r#"export default workflow({
                        "id": "status-fixture",
                        "goal": "status summary",
                        "nodes": [
                            {
                                "agent": {
                                    "id": "inspect",
                                    "prompt": "Inspect the code.",
                                    "agent_type": "review"
                                }
                            }
                        ]
                    });"#
                }),
                &ctx,
            )
            .await
            .expect("workflow run");
        let run_payload: Value = serde_json::from_str(&run.content).expect("run json");

        let status = tool
            .execute(json!({"action": "status"}), &ctx)
            .await
            .expect("workflow status");
        let status_payload: Value = serde_json::from_str(&status.content).expect("status json");
        let summary = &status_payload["runs"][0];

        assert_eq!(status_payload["count"], 1);
        assert_eq!(summary["run_id"], run_payload["run_id"]);
        assert_eq!(summary["workflow_id"], "status-fixture");
        assert_eq!(summary["workflow_goal"], "status summary");
        assert_eq!(summary["status"], "completed");
        assert_eq!(summary["execution_status"], "succeeded");
        assert_eq!(summary["child_count"], 1);
        assert_eq!(summary["leaf_count"], 1);
        assert_eq!(summary["branch_count"], 0);
        assert_eq!(summary["control_count"], 0);
        assert!(summary["event_count"].as_u64().unwrap_or_default() >= 3);
        assert_eq!(summary["last_event_type"], "run_completed");
        assert!(summary.get("result").is_none());
        assert!(summary.get("execution").is_none());
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_status_survives_tool_rebuild_via_journal() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, _calls) = fake_chat_client("journal-output").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager.clone(), runtime.clone());

        let run = tool
            .execute(
                json!({
                    "action": "run",
                    "script": "return { ok: true };"
                }),
                &ctx,
            )
            .await
            .expect("workflow run");
        let run_payload: Value = serde_json::from_str(&run.content).expect("run json");
        let run_id = run_payload["run_id"].as_str().expect("run id");

        let journal_path = tmp.path().join(".codewhale/workflow-runs.jsonl");
        assert!(
            journal_path.exists(),
            "journal should be created under workspace"
        );

        let rebuilt = WorkflowTool::new(
            manager.clone(),
            SubAgentRuntime::new(
                stub_client(),
                "deepseek-v4-flash".to_string(),
                ctx.clone(),
                true,
                None,
                manager,
            ),
        );
        let status = rebuilt
            .execute(json!({"action": "status", "run_id": run_id}), &ctx)
            .await
            .expect("workflow status after rebuild");
        let status_payload: Value = serde_json::from_str(&status.content).expect("status json");
        assert_eq!(status_payload["run_id"], run_id);
        assert_eq!(status_payload["status"], "completed");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_status_surfaces_schema_failure_instead_of_null_success() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, _calls) = fake_chat_client(r#"{"refuted":"yes"}"#).await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager, runtime);

        let run = tool
            .execute(
                json!({
                    "action": "run",
                    "script": r#"
                    return await parallel([
                        () => task({
                            description: "Return the schema fixture.",
                            responseSchema: {
                                type: "object",
                                properties: { refuted: { type: "boolean" } },
                                required: ["refuted"],
                            },
                        }),
                    ]);
                    "#
                }),
                &ctx,
            )
            .await
            .expect("workflow run returns a record");
        let run_payload: Value = serde_json::from_str(&run.content).expect("run json");

        assert_eq!(run_payload["status"], "failed");
        assert!(run_payload["result"].is_null());
        assert!(
            run_payload["error"]
                .as_str()
                .unwrap()
                .contains("responseSchema validation")
        );
        assert!(
            run_payload["progress"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message
                    .as_str()
                    .is_some_and(|message| message.contains("schema validation failed"))),
            "schema validation error should be visible in the run receipt: {run_payload}"
        );
        assert!(
            run_payload["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "task_schema_validation_failed"
                    && event["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("responseSchema validation"))),
            "schema validation event should be visible in the typed receipt: {run_payload}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn declarative_issue_audit_fixture_runs_through_subagent_driver() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let workflow_dir = tmp.path().join("workflows");
        std::fs::create_dir_all(&workflow_dir).expect("workflow dir");
        let fixture = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../workflows/issue_audit.workflow.js"),
        )
        .expect("issue audit fixture");
        std::fs::write(workflow_dir.join("issue_audit.workflow.js"), fixture)
            .expect("write fixture into workspace");

        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let (client, calls) = fake_chat_client("audited").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager,
        );
        let tool = WorkflowTool::new(runtime.manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "source_path": "workflows/issue_audit.workflow.js"
                }),
                &ctx,
            )
            .await
            .expect("declarative workflow should complete");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(payload["status"], "completed", "{payload}");
        assert_eq!(payload["result"]["code-audit"], "audited");
        assert_eq!(payload["result"]["test-audit"], "audited");
        assert_eq!(payload["result"]["docs-audit"], "audited");
        assert_eq!(payload["result"]["synthesize-release-risk"], "audited");
        assert_eq!(payload["execution"]["status"], "succeeded");
        assert_eq!(
            payload["execution"]["leaf_results"]
                .as_array()
                .expect("leaf results")
                .len(),
            3
        );
        assert_eq!(
            payload["execution"]["branch_results"][0]["branch_id"],
            "parallel-audit"
        );
        assert!(
            payload["execution"]["control_node_results"]
                .as_array()
                .expect("control results")
                .iter()
                .any(|result| result["node_id"] == "synthesize-release-risk"
                    && result["kind"] == "reduce"
                    && result["status"] == "succeeded")
        );
        assert_eq!(payload["child_ids"].as_array().unwrap().len(), 4);
        assert_eq!(calls.load(Ordering::SeqCst), 4);
        assert!(
            payload["progress"]
                .as_array()
                .unwrap()
                .iter()
                .any(|message| message == "phase: parallel-audit")
        );
    }

    #[tokio::test]
    async fn completion_from_manager_fails_closed_when_status_stays_running() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);

        let completion =
            completion_from_manager(manager, "missing_agent", "fallback".to_string()).await;
        match completion {
            TaskCompletion::Failed { message } => {
                assert!(
                    message.contains("did not report a terminal status"),
                    "{message}"
                );
            }
            other => panic!("expected timeout failure, got {other:?}"),
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_cancel_interrupts_vm_and_blocks_further_spawns() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 4);
        let (client, calls) = fake_chat_client("child done").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager.clone(), runtime);

        let started = tool
            .execute(
                json!({
                    "action": "start",
                    "script": r#"
                        let n = 0;
                        while (n < 20) {
                            await task({ description: `task ${n}`, type: 'explore', allowedTools: [] });
                            n++;
                        }
                        return n;
                    "#
                }),
                &ctx,
            )
            .await
            .expect("workflow start");
        let run_id = started
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("run_id"))
            .and_then(Value::as_str)
            .expect("run_id metadata");

        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        let calls_before_cancel = calls.load(Ordering::SeqCst);
        assert!(
            calls_before_cancel >= 1,
            "workflow should spawn at least one child before cancel"
        );

        let cancelled = tool
            .execute(json!({"action": "cancel", "run_id": run_id}), &ctx)
            .await
            .expect("workflow cancel");
        let cancelled_payload: Value =
            serde_json::from_str(&cancelled.content).expect("cancel json");
        assert_eq!(cancelled_payload["status"], "cancelled");

        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        let calls_after_cancel = calls.load(Ordering::SeqCst);
        assert!(
            calls_after_cancel <= calls_before_cancel + 1,
            "cancelled workflow kept spawning children: before={calls_before_cancel} after={calls_after_cancel}"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn workflow_budget_spent_delegates_to_manager_scope() {
        let _retry_guard = workflow_test_retry_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let ctx = ToolContext::new(tmp.path().to_path_buf());
        let manager = new_shared_subagent_manager(tmp.path().to_path_buf(), 2);
        let (client, _calls) = fake_chat_client("budgeted").await;
        let runtime = SubAgentRuntime::new(
            client,
            "deepseek-v4-flash".to_string(),
            ctx.clone(),
            true,
            None,
            manager.clone(),
        );
        let tool = WorkflowTool::new(manager.clone(), runtime);

        let result = tool
            .execute(
                json!({
                    "action": "run",
                    "token_budget": 1000,
                    "script": r#"
                        await task({ description: 'budgeted work', type: 'explore', allowedTools: [] });
                        return { spent: budget.spent(), total: budget.total, remaining: budget.remaining() };
                    "#
                }),
                &ctx,
            )
            .await
            .expect("budget workflow should complete");
        let payload: Value = serde_json::from_str(&result.content).expect("json result");

        assert_eq!(payload["status"], "completed", "{payload}");
        assert_eq!(payload["result"]["spent"], 2);
        assert_eq!(payload["result"]["total"], 1000);
        assert_eq!(payload["result"]["remaining"], 998);
    }

    fn stub_client() -> DeepSeekClient {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = crate::config::Config {
            api_key: Some("test-key".to_string()),
            ..crate::config::Config::default()
        };
        DeepSeekClient::new(&config).expect("stub client should construct")
    }

    async fn fake_chat_client(response_text: &str) -> (DeepSeekClient, Arc<AtomicUsize>) {
        let (client, calls, _) = fake_chat_client_capturing(response_text).await;
        (client, calls)
    }

    async fn fake_chat_client_capturing(
        response_text: &str,
    ) -> (DeepSeekClient, Arc<AtomicUsize>, Arc<Mutex<Vec<Value>>>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let response_text = response_text.to_string();
        let app = Router::new().route(
            "/{*path}",
            post({
                let calls = Arc::clone(&calls);
                let bodies = Arc::clone(&bodies);
                move |Json(body): Json<Value>| {
                    let calls = Arc::clone(&calls);
                    let bodies = Arc::clone(&bodies);
                    let response_text = response_text.clone();
                    async move {
                        bodies.lock().expect("capture body").push(body);
                        let attempt = calls.fetch_add(1, Ordering::SeqCst) + 1;
                        Json(json!({
                            "id": format!("chatcmpl-workflow-test-{attempt}"),
                            "model": "deepseek-v4-flash",
                            "choices": [{
                                "index": 0,
                                "message": {
                                    "role": "assistant",
                                    "content": response_text
                                },
                                "finish_reason": "stop"
                            }],
                            "usage": {
                                "prompt_tokens": 1,
                                "completion_tokens": 1,
                                "total_tokens": 2
                            }
                        }))
                    }
                }
            }),
        );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake chat server");
        let addr = listener.local_addr().expect("fake chat server addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let config = crate::config::Config {
            api_key: Some("test-key".to_string()),
            base_url: Some(format!("http://{addr}/v1")),
            ..crate::config::Config::default()
        };
        (
            DeepSeekClient::new(&config).expect("fake chat client"),
            calls,
            bodies,
        )
    }

    fn workflow_test_retry_guard() -> std::sync::MutexGuard<'static, ()> {
        let guard = crate::retry_status::test_guard();
        crate::retry_status::clear();
        crate::retry_status::clear_rate_limit();
        guard
    }
}
