//! Persistent background task manager for Codewhale agent work.
//!
//! Tasks are durable across restarts and execute with a bounded worker pool.
//! Execution stays DeepSeek-only and now links every task to runtime
//! thread/turn records for unified timelines.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration as StdDuration;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::Config;
use crate::runtime_threads::{
    CreateThreadRequest, RuntimeEventRecord, RuntimeThreadManager, RuntimeThreadManagerConfig,
    RuntimeTurnStatus, SharedRuntimeThreadManager, StartTurnRequest,
};
use crate::utils::spawn_supervised;

const DEFAULT_WORKERS: usize = 2;
const MAX_WORKERS: usize = 8;
const TIMELINE_SUMMARY_LIMIT: usize = 240;
const TIMELINE_ENTRY_LIMIT: usize = 256;
const TIMELINE_HEAD_KEEP: usize = 8;
const ARTIFACT_THRESHOLD: usize = 1200;
const TASK_EVENT_CHANNEL_CAPACITY: usize = 256;
const EVENT_CURSOR_BATCH: usize = 256;
const EVENT_CATCHUP_POLL: Duration = Duration::from_millis(200);
// `lifecycle_seq` is an additive, serde-defaulted field. Keep the durable task
// schema at v2 so a v0.9.1 rollback can ignore it and still open tasks written
// by this build; no existing field changed meaning.
const CURRENT_TASK_SCHEMA_VERSION: u32 = 2;

const fn default_task_schema_version() -> u32 {
    CURRENT_TASK_SCHEMA_VERSION
}

/// Durable task status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

/// What the manager actually did while handling a cancellation request.
///
/// This is returned from the same state-lock transaction as the task record,
/// so callers never have to infer an outcome from a stale pre-cancel read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskCancelDisposition {
    Forced,
    Requested,
    AlreadyFinished,
}

#[derive(Debug, Clone)]
pub struct TaskCancellation {
    pub task: TaskRecord,
    pub disposition: TaskCancelDisposition,
}

impl TaskStatus {
    #[cfg(test)]
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
    }
}

/// Why a durable task left the running state. Stored on the task record so
/// receipts and status views can show a forced timeout separately from a
/// cooperative cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskTerminalReason {
    Completed,
    Canceled,
    CancelTimeout,
    Shutdown,
    WallTimeout,
    IdleTimeout,
    Failed,
}

impl TaskTerminalReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Canceled => "canceled",
            Self::CancelTimeout => "cancel_timeout",
            Self::Shutdown => "shutdown",
            Self::WallTimeout => "wall_timeout",
            Self::IdleTimeout => "idle_timeout",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub const fn task_status(self) -> TaskStatus {
        match self {
            Self::Completed => TaskStatus::Completed,
            Self::Canceled | Self::CancelTimeout | Self::Shutdown => TaskStatus::Canceled,
            Self::WallTimeout | Self::IdleTimeout | Self::Failed => TaskStatus::Failed,
        }
    }

    #[must_use]
    pub fn receipt_message(self) -> String {
        match self {
            Self::Completed => "Task completed".to_string(),
            Self::Canceled => "Task canceled".to_string(),
            Self::CancelTimeout => {
                "Task did not terminalize after cancellation; worker released".to_string()
            }
            Self::Shutdown => "Task canceled because the task manager shut down".to_string(),
            Self::WallTimeout => {
                "Task exceeded its wall-time deadline without completing".to_string()
            }
            Self::IdleTimeout => {
                "Task made no model or tool progress before the idle deadline".to_string()
            }
            Self::Failed => "Task ended unexpectedly".to_string(),
        }
    }

    fn after_grace(self) -> Self {
        match self {
            Self::Canceled => Self::CancelTimeout,
            other => other,
        }
    }
}

/// Durable tool-call status within a task timeline.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskToolStatus {
    Running,
    Success,
    Failed,
    Canceled,
}

/// Timeline entry for a task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskTimelineEntry {
    pub timestamp: DateTime<Utc>,
    pub kind: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_path: Option<PathBuf>,
}

/// Tool call summary for a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskToolCallSummary {
    pub id: String,
    pub name: String,
    pub status: TaskToolStatus,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_ref: Option<PathBuf>,
}

/// Checklist item stored on durable tasks. This is the durable form behind the
/// model-visible checklist/todo compatibility tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskChecklistItem {
    pub id: u32,
    pub content: String,
    pub status: String,
}

/// Checklist state associated with a task.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskChecklistState {
    pub items: Vec<TaskChecklistItem>,
    pub completion_pct: u8,
    pub in_progress_id: Option<u32>,
    pub updated_at: Option<DateTime<Utc>>,
}

/// Structured verification evidence attached to a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGateRecord {
    pub id: String,
    pub gate: String,
    pub command: String,
    pub cwd: PathBuf,
    pub exit_code: Option<i32>,
    pub status: String,
    pub classification: String,
    pub duration_ms: u64,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<PathBuf>,
    pub recorded_at: DateTime<Utc>,
}

/// PR-attempt metadata and artifacts attached to a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskAttemptRecord {
    pub id: String,
    pub attempt_group_id: String,
    pub attempt_index: u32,
    pub attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    pub summary: String,
    pub changed_files: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch_path: Option<PathBuf>,
    pub verification: Vec<String>,
    pub selected: bool,
    pub recorded_at: DateTime<Utc>,
}

/// Durable artifact reference produced by task-aware tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskArtifactRef {
    pub label: String,
    pub path: PathBuf,
    pub summary: String,
    pub created_at: DateTime<Utc>,
}

/// GitHub write/read evidence attached to a task timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGithubEvent {
    pub id: String,
    pub action: String,
    pub target: String,
    pub number: u64,
    pub summary: String,
    pub url: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

/// Durable task record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    #[serde(default = "default_task_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub prompt: String,
    pub model: String,
    pub workspace: PathBuf,
    pub mode: String,
    pub allow_shell: bool,
    pub trust_mode: bool,
    #[serde(default = "default_auto_approve")]
    pub auto_approve: bool,
    pub status: TaskStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunt_verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_detail_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
    #[serde(default)]
    pub runtime_event_count: usize,
    /// Monotonic owner-lifecycle sequence used by Work Graph reconciliation.
    /// Output/progress events do not advance this counter; only lifecycle
    /// transitions do, so replay after restart is stable.
    #[serde(default)]
    pub lifecycle_seq: u64,
    #[serde(default)]
    pub checklist: TaskChecklistState,
    #[serde(default)]
    pub gates: Vec<TaskGateRecord>,
    #[serde(default)]
    pub attempts: Vec<TaskAttemptRecord>,
    #[serde(default)]
    pub artifacts: Vec<TaskArtifactRef>,
    #[serde(default)]
    pub github_events: Vec<TaskGithubEvent>,
    pub tool_calls: Vec<TaskToolCallSummary>,
    pub timeline: Vec<TaskTimelineEntry>,
}

/// Lightweight task view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub status: TaskStatus,
    pub prompt_summary: String,
    pub model: String,
    pub mode: String,
    pub workspace: PathBuf,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub lifecycle_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hunt_verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_id: Option<String>,
}

impl From<&TaskRecord> for TaskSummary {
    fn from(value: &TaskRecord) -> Self {
        Self {
            id: value.id.clone(),
            status: value.status,
            prompt_summary: summarize_text(&value.prompt, TIMELINE_SUMMARY_LIMIT),
            model: value.model.clone(),
            mode: value.mode.clone(),
            workspace: value.workspace.clone(),
            created_at: value.created_at,
            started_at: value.started_at,
            ended_at: value.ended_at,
            duration_ms: value.duration_ms,
            lifecycle_seq: value.lifecycle_seq,
            hunt_verdict: value.hunt_verdict.clone(),
            error: value.error.clone(),
            terminal_reason: value.terminal_reason.clone(),
            thread_id: value.thread_id.clone(),
            turn_id: value.turn_id.clone(),
            owner_session_id: value.owner_session_id.clone(),
        }
    }
}

/// Count totals by status for task dashboards.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct TaskCounts {
    pub queued: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
    pub canceled: usize,
}

/// Request to enqueue a new task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewTaskRequest {
    pub prompt: String,
    pub model: Option<String>,
    pub workspace: Option<PathBuf>,
    pub mode: Option<String>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
    pub owner_session_id: Option<String>,
}

impl NewTaskRequest {
    #[cfg(test)]
    #[must_use]
    pub fn from_prompt(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: Some(true),
            owner_session_id: None,
        }
    }
}

/// Task manager startup options.
#[derive(Debug, Clone)]
pub struct TaskManagerConfig {
    pub data_dir: PathBuf,
    pub worker_count: usize,
    pub default_workspace: PathBuf,
    pub default_model: String,
    pub default_mode: String,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub execution_limits: TaskExecutionLimits,
}

/// Deadlines and persistence cadence for one durable execution.
#[derive(Debug, Clone, Copy)]
pub struct TaskExecutionLimits {
    pub wall_time: Duration,
    pub idle_progress: Duration,
    pub cancel_grace: Duration,
    pub persist_debounce: Duration,
}

impl Default for TaskExecutionLimits {
    fn default() -> Self {
        Self {
            wall_time: Duration::from_secs(30 * 60),
            idle_progress: Duration::from_secs(2 * 60),
            cancel_grace: Duration::from_secs(5),
            persist_debounce: Duration::from_millis(250),
        }
    }
}

#[cfg(test)]
impl TaskExecutionLimits {
    fn short_for_tests() -> Self {
        Self {
            wall_time: Duration::from_millis(400),
            idle_progress: Duration::from_millis(150),
            cancel_grace: Duration::from_millis(50),
            persist_debounce: Duration::from_millis(10),
        }
    }
}

/// Pure watchdog for cancel grace, wall time, and idle progress.
struct ExecutionGuard {
    started_at: Instant,
    last_progress_at: Instant,
    interrupt_at: Option<Instant>,
    interrupt_reason: Option<TaskTerminalReason>,
    limits: TaskExecutionLimits,
}

#[derive(Debug)]
enum GuardAction {
    Run { wait: Duration },
    Interrupt { reason: TaskTerminalReason },
    Terminalize { reason: TaskTerminalReason },
}

impl ExecutionGuard {
    fn new(limits: TaskExecutionLimits, now: Instant) -> Self {
        Self {
            started_at: now,
            last_progress_at: now,
            interrupt_at: None,
            interrupt_reason: None,
            limits,
        }
    }

    fn note_progress(&mut self, now: Instant) {
        self.last_progress_at = now;
    }

    fn note_interrupt(&mut self, now: Instant, reason: TaskTerminalReason) {
        if self.interrupt_at.is_none() {
            self.interrupt_at = Some(now);
            self.interrupt_reason = Some(reason);
        }
    }

    fn evaluate(&self, now: Instant, cancel: bool, shutdown: bool) -> GuardAction {
        if let Some(interrupt_at) = self.interrupt_at {
            let elapsed = now.saturating_duration_since(interrupt_at);
            if elapsed >= self.limits.cancel_grace {
                let reason = self
                    .interrupt_reason
                    .unwrap_or(TaskTerminalReason::CancelTimeout)
                    .after_grace();
                return GuardAction::Terminalize { reason };
            }
            return GuardAction::Run {
                wait: self.limits.cancel_grace.saturating_sub(elapsed),
            };
        }

        let wall_elapsed = now.saturating_duration_since(self.started_at);
        let idle_elapsed = now.saturating_duration_since(self.last_progress_at);
        let pending = if shutdown {
            Some(TaskTerminalReason::Shutdown)
        } else if cancel {
            Some(TaskTerminalReason::Canceled)
        } else if wall_elapsed >= self.limits.wall_time {
            Some(TaskTerminalReason::WallTimeout)
        } else if idle_elapsed >= self.limits.idle_progress {
            Some(TaskTerminalReason::IdleTimeout)
        } else {
            None
        };
        if let Some(reason) = pending {
            return GuardAction::Interrupt { reason };
        }

        let wait = self
            .limits
            .wall_time
            .saturating_sub(wall_elapsed)
            .min(self.limits.idle_progress.saturating_sub(idle_elapsed))
            .min(EVENT_CATCHUP_POLL);
        GuardAction::Run {
            wait: wait.max(Duration::from_millis(1)),
        }
    }

    fn preserve_timeout_reason(&self, result: TaskExecutionResult) -> TaskExecutionResult {
        if result.terminal_reason != TaskTerminalReason::Canceled {
            return result;
        }
        match self.interrupt_reason {
            Some(reason @ (TaskTerminalReason::WallTimeout | TaskTerminalReason::IdleTimeout)) => {
                TaskExecutionResult::from_reason(reason, result.result_text)
            }
            _ => result,
        }
    }
}

impl TaskManagerConfig {
    #[must_use]
    pub fn from_runtime(
        config: &Config,
        workspace: PathBuf,
        default_model: Option<String>,
        worker_count: Option<usize>,
    ) -> Self {
        Self {
            data_dir: default_tasks_dir(),
            worker_count: worker_count.unwrap_or(DEFAULT_WORKERS),
            default_workspace: workspace,
            default_model: default_model.unwrap_or_else(|| config.default_model()),
            default_mode: "agent".to_string(),
            allow_shell: config.allow_shell(),
            trust_mode: false,
            execution_limits: TaskExecutionLimits::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionTask {
    id: String,
    prompt: String,
    model: String,
    workspace: PathBuf,
    mode_label: String,
    allow_shell: bool,
    trust_mode: bool,
    auto_approve: bool,
}

/// Event stream produced by an executor while a task runs.
#[derive(Debug, Clone)]
pub enum TaskExecutionEvent {
    ThreadLinked {
        thread_id: String,
        turn_id: String,
    },
    Status {
        message: String,
    },
    MessageDelta {
        content: String,
    },
    ToolStarted {
        id: String,
        name: String,
        input: Value,
    },
    ToolProgress {
        id: String,
        output: String,
    },
    ToolCompleted {
        id: String,
        name: String,
        success: bool,
        output: String,
        metadata: Option<Value>,
    },
    Error {
        message: String,
    },
    RuntimeEvent {
        seq: u64,
        event: String,
        summary: String,
    },
}

/// Final executor result.
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    pub status: TaskStatus,
    pub result_text: Option<String>,
    pub error: Option<String>,
    pub terminal_reason: TaskTerminalReason,
}

impl TaskExecutionResult {
    fn failed(error: impl Into<String>) -> Self {
        Self {
            status: TaskStatus::Failed,
            result_text: None,
            error: Some(error.into()),
            terminal_reason: TaskTerminalReason::Failed,
        }
    }

    fn from_reason(reason: TaskTerminalReason, result_text: Option<String>) -> Self {
        let error = match reason {
            TaskTerminalReason::Completed | TaskTerminalReason::Canceled => None,
            _ => Some(reason.receipt_message()),
        };
        Self {
            status: reason.task_status(),
            result_text,
            error,
            terminal_reason: reason,
        }
    }
}

/// Abstraction for task execution.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::Sender<TaskExecutionEvent>,
        cancel: CancellationToken,
    ) -> TaskExecutionResult;
}

/// Engine-backed executor (DeepSeek-only).
pub struct EngineTaskExecutor {
    runtime_threads: SharedRuntimeThreadManager,
    limits: TaskExecutionLimits,
}

impl EngineTaskExecutor {
    #[must_use]
    pub fn new(runtime_threads: SharedRuntimeThreadManager, limits: TaskExecutionLimits) -> Self {
        Self {
            runtime_threads,
            limits,
        }
    }
}

#[async_trait]
impl TaskExecutor for EngineTaskExecutor {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::Sender<TaskExecutionEvent>,
        cancel: CancellationToken,
    ) -> TaskExecutionResult {
        let thread = match self
            .runtime_threads
            .create_thread(CreateThreadRequest {
                model: Some(task.model.clone()),
                workspace: Some(task.workspace.clone()),
                mode: Some(task.mode_label.clone()),
                allow_shell: Some(task.allow_shell),
                trust_mode: Some(task.trust_mode),
                auto_approve: Some(task.auto_approve),
                archived: false,
                system_prompt: None,
                task_id: Some(task.id.clone()),
                ..Default::default()
            })
            .await
        {
            Ok(thread) => thread,
            Err(err) => {
                return TaskExecutionResult::failed(format!(
                    "Failed to create runtime thread: {err}"
                ));
            }
        };

        let turn = match self
            .runtime_threads
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: task.prompt.clone(),
                    input_summary: Some(summarize_text(&task.prompt, TIMELINE_SUMMARY_LIMIT)),
                    model: Some(task.model.clone()),
                    mode: Some(task.mode_label.clone()),
                    allow_shell: Some(task.allow_shell),
                    trust_mode: Some(task.trust_mode),
                    auto_approve: Some(task.auto_approve),
                    ..Default::default()
                },
            )
            .await
        {
            Ok(turn) => turn,
            Err(err) => {
                return TaskExecutionResult::failed(format!("Failed to start task: {err}"));
            }
        };

        emit_task_event(
            &events,
            TaskExecutionEvent::ThreadLinked {
                thread_id: thread.id.clone(),
                turn_id: turn.id.clone(),
            },
        )
        .await;
        emit_task_event(
            &events,
            TaskExecutionEvent::Status {
                message: format!("Task {} started", task.id),
            },
        )
        .await;

        drive_engine_turn(
            self.runtime_threads.as_ref(),
            &thread.id,
            &turn.id,
            events,
            cancel,
            self.limits,
        )
        .await
    }
}

async fn drive_engine_turn(
    runtime_threads: &RuntimeThreadManager,
    thread_id: &str,
    turn_id: &str,
    events: mpsc::Sender<TaskExecutionEvent>,
    cancel: CancellationToken,
    limits: TaskExecutionLimits,
) -> TaskExecutionResult {
    let mut subscription = runtime_threads.subscribe_events();
    let mut guard = ExecutionGuard::new(limits, Instant::now());
    let mut final_text = String::new();
    let mut cursor = 0u64;
    let mut terminal_status: Option<RuntimeTurnStatus> = None;
    let mut terminal_error: Option<String> = None;

    loop {
        let batch = match runtime_threads
            .events_from_offset_async(thread_id, cursor, Some(EVENT_CURSOR_BATCH))
            .await
        {
            Ok((batch, next_cursor)) => {
                cursor = next_cursor;
                batch
            }
            Err(err) => {
                return TaskExecutionResult {
                    status: TaskStatus::Failed,
                    result_text: optional_nonzero_text(final_text),
                    error: Some(format!("Failed to read runtime events: {err}")),
                    terminal_reason: TaskTerminalReason::Failed,
                };
            }
        };

        let more_pending = batch.len() >= EVENT_CURSOR_BATCH;
        for event in batch {
            if event.thread_id != thread_id {
                continue;
            }
            if event
                .turn_id
                .as_deref()
                .is_some_and(|event_turn| event_turn != turn_id)
            {
                continue;
            }
            if runtime_event_is_progress(&event) {
                guard.note_progress(Instant::now());
            }
            if let Some((status, error)) =
                ingest_runtime_event(&event, &mut final_text, &events).await
            {
                terminal_status = Some(status);
                terminal_error = error;
            }
        }

        if terminal_status.is_some() {
            break;
        }

        match guard.evaluate(Instant::now(), cancel.is_cancelled(), false) {
            GuardAction::Interrupt { reason } => {
                let _ = runtime_threads.interrupt_turn(thread_id, turn_id).await;
                emit_task_event(
                    &events,
                    TaskExecutionEvent::Status {
                        message: reason.receipt_message(),
                    },
                )
                .await;
                guard.note_interrupt(Instant::now(), reason);
            }
            GuardAction::Terminalize { reason } => {
                return TaskExecutionResult::from_reason(reason, optional_nonzero_text(final_text));
            }
            GuardAction::Run { wait } => {
                if more_pending {
                    continue;
                }
                tokio::select! {
                    _ = cancel.cancelled(), if !cancel.is_cancelled() => {}
                    _ = subscription.recv() => {}
                    _ = sleep(wait) => {}
                }
            }
        }
    }

    let result = match terminal_status.unwrap_or(RuntimeTurnStatus::Failed) {
        RuntimeTurnStatus::Completed => TaskExecutionResult {
            status: TaskStatus::Completed,
            result_text: optional_nonzero_text(final_text),
            error: None,
            terminal_reason: TaskTerminalReason::Completed,
        },
        RuntimeTurnStatus::Interrupted | RuntimeTurnStatus::Canceled => TaskExecutionResult {
            status: TaskStatus::Canceled,
            result_text: optional_nonzero_text(final_text),
            error: None,
            terminal_reason: TaskTerminalReason::Canceled,
        },
        RuntimeTurnStatus::Queued | RuntimeTurnStatus::InProgress | RuntimeTurnStatus::Failed => {
            TaskExecutionResult {
                status: TaskStatus::Failed,
                result_text: optional_nonzero_text(final_text),
                error: terminal_error
                    .or_else(|| Some(TaskTerminalReason::Failed.receipt_message())),
                terminal_reason: TaskTerminalReason::Failed,
            }
        }
    };
    guard.preserve_timeout_reason(result)
}

async fn emit_task_event(events: &mpsc::Sender<TaskExecutionEvent>, event: TaskExecutionEvent) {
    let _ = events.send(event).await;
}

fn optional_nonzero_text(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn append_message_delta(result_text: &mut String, event: &TaskExecutionEvent) {
    if let TaskExecutionEvent::MessageDelta { content } = event {
        result_text.push_str(content);
    }
}

fn runtime_event_is_progress(event: &RuntimeEventRecord) -> bool {
    matches!(
        event.event.as_str(),
        "item.delta" | "item.started" | "item.completed" | "item.failed" | "turn.completed"
    )
}

async fn ingest_runtime_event(
    event: &RuntimeEventRecord,
    final_text: &mut String,
    events: &mpsc::Sender<TaskExecutionEvent>,
) -> Option<(RuntimeTurnStatus, Option<String>)> {
    emit_task_event(
        events,
        TaskExecutionEvent::RuntimeEvent {
            seq: event.seq,
            event: event.event.clone(),
            summary: summarize_text(&event.payload.to_string(), TIMELINE_SUMMARY_LIMIT),
        },
    )
    .await;

    match event.event.as_str() {
        "item.delta" => {
            let kind = event
                .payload
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if kind == "agent_message" {
                if let Some(content) = event.payload.get("delta").and_then(Value::as_str) {
                    final_text.push_str(content);
                    emit_task_event(
                        events,
                        TaskExecutionEvent::MessageDelta {
                            content: content.to_string(),
                        },
                    )
                    .await;
                }
            } else if kind == "tool_call" {
                let output = event
                    .payload
                    .get("delta")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                emit_task_event(
                    events,
                    TaskExecutionEvent::ToolProgress {
                        id: event.item_id.clone().unwrap_or_default(),
                        output,
                    },
                )
                .await;
            }
            None
        }
        "item.started" => {
            if let Some(tool) = event.payload.get("tool") {
                let id = tool
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let input = tool.get("input").cloned().unwrap_or_else(|| json!({}));
                emit_task_event(events, TaskExecutionEvent::ToolStarted { id, name, input }).await;
            }
            None
        }
        "item.completed" | "item.failed" => {
            if let Some(item) = event.payload.get("item") {
                let kind = item.get("kind").and_then(Value::as_str).unwrap_or_default();
                if kind == "tool_call" || kind == "file_change" || kind == "command_execution" {
                    let id = item
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let name = item
                        .get("summary")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .split(':')
                        .next()
                        .unwrap_or("tool")
                        .trim()
                        .to_string();
                    let output = item
                        .get("detail")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let metadata = item.get("metadata").cloned();
                    emit_task_event(
                        events,
                        TaskExecutionEvent::ToolCompleted {
                            id,
                            name,
                            success: event.event == "item.completed",
                            output,
                            metadata,
                        },
                    )
                    .await;
                } else if kind == "status" {
                    let message = item
                        .get("detail")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("summary").and_then(Value::as_str))
                        .unwrap_or_default()
                        .to_string();
                    emit_task_event(events, TaskExecutionEvent::Status { message }).await;
                } else if kind == "error" {
                    let message = item
                        .get("detail")
                        .and_then(Value::as_str)
                        .or_else(|| item.get("summary").and_then(Value::as_str))
                        .unwrap_or_default()
                        .to_string();
                    emit_task_event(events, TaskExecutionEvent::Error { message }).await;
                }
            }
            None
        }
        "turn.completed" => {
            if let Some(turn_payload) = event.payload.get("turn") {
                let status = turn_payload
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                let terminal_status = match status {
                    "completed" => RuntimeTurnStatus::Completed,
                    "interrupted" => RuntimeTurnStatus::Interrupted,
                    "canceled" => RuntimeTurnStatus::Canceled,
                    _ => RuntimeTurnStatus::Failed,
                };
                let terminal_error = turn_payload
                    .get("error")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                Some((terminal_status, terminal_error))
            } else {
                Some((RuntimeTurnStatus::Completed, None))
            }
        }
        _ => None,
    }
}

/// Thread-safe task manager.
pub type SharedTaskManager = Arc<TaskManager>;

pub struct TaskManager {
    cfg: TaskManagerConfig,
    default_workspace: Mutex<PathBuf>,
    executor: Arc<dyn TaskExecutor>,
    tasks_dir: PathBuf,
    artifacts_dir: PathBuf,
    queue_path: PathBuf,
    state: Mutex<ManagerState>,
    notify: Notify,
    cancel_token: CancellationToken,
}

struct ManagerState {
    tasks: HashMap<String, TaskRecord>,
    queue: VecDeque<String>,
    running_cancel: HashMap<String, CancellationToken>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct QueueFile {
    queue: Vec<String>,
}

impl TaskManager {
    /// Start the manager with the default DeepSeek executor.
    ///
    /// Interactive callers pass the session id so the Runtime store (and its
    /// exclusive process-owner lock) is per-session rather than per-machine
    /// (#5630).
    pub async fn start(
        cfg: TaskManagerConfig,
        api_config: Config,
        plugin_registry: Arc<crate::plugins::PluginRegistry>,
        session_id: &str,
    ) -> Result<SharedTaskManager> {
        let runtime_threads = Arc::new(RuntimeThreadManager::open_with_plugin_registry(
            api_config.clone(),
            cfg.default_workspace.clone(),
            RuntimeThreadManagerConfig::for_session(cfg.data_dir.clone(), session_id),
            plugin_registry,
        )?);
        Self::start_with_runtime_manager(cfg, api_config, runtime_threads).await
    }

    /// Start the manager with an injected runtime thread manager.
    pub async fn start_with_runtime_manager(
        cfg: TaskManagerConfig,
        _api_config: Config,
        runtime_threads: SharedRuntimeThreadManager,
    ) -> Result<SharedTaskManager> {
        let executor: Arc<dyn TaskExecutor> = Arc::new(EngineTaskExecutor::new(
            runtime_threads.clone(),
            cfg.execution_limits,
        ));
        let manager = Self::start_with_executor(cfg, executor).await?;
        runtime_threads.attach_task_manager(manager.clone());
        Ok(manager)
    }

    /// Start the manager with a custom executor (used for tests).
    pub async fn start_with_executor(
        cfg: TaskManagerConfig,
        executor: Arc<dyn TaskExecutor>,
    ) -> Result<SharedTaskManager> {
        let workers = cfg.worker_count.clamp(1, MAX_WORKERS);
        let tasks_dir = cfg.data_dir.join("tasks");
        let artifacts_dir = cfg.data_dir.join("artifacts");
        let queue_path = cfg.data_dir.join("queue.json");
        fs::create_dir_all(&tasks_dir)
            .with_context(|| format!("Failed to create tasks dir {}", tasks_dir.display()))?;
        fs::create_dir_all(&artifacts_dir).with_context(|| {
            format!(
                "Failed to create task artifacts dir {}",
                artifacts_dir.display()
            )
        })?;

        let LoadedTaskState {
            tasks,
            queue,
            recovered,
        } = load_state(&tasks_dir, &queue_path)?;

        let cancel_token = CancellationToken::new();
        let default_workspace = cfg.default_workspace.clone();
        let manager = Arc::new(Self {
            cfg,
            default_workspace: Mutex::new(default_workspace),
            executor,
            tasks_dir,
            artifacts_dir,
            queue_path,
            state: Mutex::new(ManagerState {
                tasks,
                queue,
                running_cancel: HashMap::new(),
            }),
            notify: Notify::new(),
            cancel_token: cancel_token.clone(),
        });

        {
            // Persist only what boot actually changed: the reconciled queue
            // and any running->failed recoveries. Rewriting every task record
            // on every launch was a full-store write storm (#3757).
            let state = manager.state.lock().await;
            manager.persist_queue_locked(&state.queue)?;
            for id in &recovered {
                if let Some(task) = state.tasks.get(id) {
                    manager.persist_task_locked(task)?;
                }
            }
        }

        for _ in 0..workers {
            let manager_clone = Arc::clone(&manager);
            spawn_supervised(
                "task-manager-worker",
                std::panic::Location::caller(),
                async move {
                    manager_clone.worker_loop().await;
                },
            );
        }

        Ok(manager)
    }

    /// Only exercised from automation_manager tests today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    pub async fn set_default_workspace(&self, workspace: PathBuf) {
        let mut default_workspace = self.default_workspace.lock().await;
        *default_workspace = workspace;
    }

    pub async fn default_workspace(&self) -> PathBuf {
        self.default_workspace.lock().await.clone()
    }

    /// Enqueue a new task.
    pub async fn add_task(&self, req: NewTaskRequest) -> Result<TaskRecord> {
        self.add_task_with_id(req, Self::new_task_id()).await
    }

    /// Allocate the durable owner identity before queue insertion so callers
    /// can register graph spawn intent first.
    #[must_use]
    pub(crate) fn new_task_id() -> String {
        format!("task_{}", &Uuid::new_v4().simple().to_string()[..16])
    }

    /// Enqueue using a preallocated id. This is crate-visible only for the
    /// model tool's register-before-work transaction.
    pub(crate) async fn add_task_with_id(
        &self,
        req: NewTaskRequest,
        task_id: String,
    ) -> Result<TaskRecord> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("Task prompt cannot be empty");
        }
        if task_id.len() != 21
            || !task_id.starts_with("task_")
            || !task_id[5..].chars().all(|ch| ch.is_ascii_hexdigit())
        {
            bail!("Invalid preallocated task id: expected task_<16hex>");
        }

        let task = TaskRecord {
            schema_version: CURRENT_TASK_SCHEMA_VERSION,
            // 16 random hex chars (was 8; ~60 bits of entropy once UUIDv4's
            // fixed version nibble is discounted): task ids live in durable
            // state that accumulates across restarts, and a collision
            // overwrites a record while leaving a duplicate queue entry.
            // `resolve_task_id` matches by prefix, so short references still
            // work.
            id: task_id,
            prompt,
            model: req.model.unwrap_or_else(|| self.cfg.default_model.clone()),
            workspace: match req.workspace {
                Some(workspace) => workspace,
                None => self.default_workspace().await,
            },
            mode: req.mode.unwrap_or_else(|| self.cfg.default_mode.clone()),
            allow_shell: req.allow_shell.unwrap_or(self.cfg.allow_shell),
            trust_mode: req.trust_mode.unwrap_or(self.cfg.trust_mode),
            // Auto-approval must be opted into explicitly
            // (GHSA-72w5-pf8h-xfp4).
            auto_approve: req.auto_approve.unwrap_or(false),
            status: TaskStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            ended_at: None,
            duration_ms: None,
            hunt_verdict: None,
            result_summary: None,
            result_detail_path: None,
            error: None,
            terminal_reason: None,
            thread_id: None,
            turn_id: None,
            owner_session_id: req.owner_session_id,
            runtime_event_count: 0,
            lifecycle_seq: 1,
            checklist: TaskChecklistState::default(),
            gates: Vec::new(),
            attempts: Vec::new(),
            artifacts: Vec::new(),
            github_events: Vec::new(),
            tool_calls: Vec::new(),
            timeline: vec![TaskTimelineEntry {
                timestamp: Utc::now(),
                kind: "queued".to_string(),
                summary: "Task queued".to_string(),
                detail_path: None,
            }],
        };

        {
            let mut state = self.state.lock().await;
            let task_path = self.tasks_dir.join(format!("{}.json", task.id));
            // The staged extension is intentionally not `.json`, so startup
            // replay ignores an interrupted create until the queue write has
            // succeeded and this file is atomically promoted.
            let staged_task_path = self.tasks_dir.join(format!(".{}.json.pending", task.id));
            if state.tasks.contains_key(&task.id) || task_path.exists() || staged_task_path.exists()
            {
                bail!("Task id already exists: {}", task.id);
            }
            let mut next_queue = state.queue.clone();
            next_queue.push_back(task.id.clone());

            // Stage the owner record, then persist its queue membership, then
            // atomically promote it. A crash before promotion leaves either an
            // ignored staged file or a queue entry with no task (which replay
            // drops); a crash after promotion leaves the complete runnable
            // pair. In-memory scheduling is published only after all three.
            write_json_atomic(&staged_task_path, &task)?;
            if let Err(err) = self.persist_queue_locked(&next_queue) {
                if let Err(cleanup_err) = fs::remove_file(&staged_task_path) {
                    tracing::warn!(
                        task_id = %task.id,
                        error = %cleanup_err,
                        "failed to remove ignored staged task after queue write failure"
                    );
                }
                return Err(err);
            }
            if let Err(promote_err) = fs::rename(&staged_task_path, &task_path) {
                let rollback_error = self.persist_queue_locked(&state.queue).err();
                let cleanup_error = fs::remove_file(&staged_task_path).err();
                let mut message =
                    format!("Failed to promote staged task {}: {promote_err}", task.id);
                if let Some(rollback_error) = rollback_error {
                    message.push_str(&format!("; queue rollback also failed: {rollback_error:#}"));
                }
                if let Some(cleanup_error) = cleanup_error {
                    message.push_str(&format!(
                        "; ignored staged-file cleanup also failed: {cleanup_error}"
                    ));
                }
                bail!(message);
            }
            state.queue = next_queue;
            state.tasks.insert(task.id.clone(), task.clone());
        }
        self.notify.notify_one();
        Ok(task)
    }

    /// List tasks, newest first.
    pub async fn list_tasks(&self, limit: Option<usize>) -> Vec<TaskSummary> {
        self.list_tasks_scoped(limit, None).await
    }

    /// List tasks, newest first, optionally scoped to a workspace.
    pub async fn list_tasks_scoped(
        &self,
        limit: Option<usize>,
        workspace: Option<&Path>,
    ) -> Vec<TaskSummary> {
        self.list_tasks_visible_to(limit, workspace, None).await
    }

    /// List tasks owned by a session, newest first, optionally scoped to a workspace.
    ///
    /// Ownerless legacy records fail closed and are not model-visible.
    pub async fn list_tasks_for_owner(
        &self,
        limit: Option<usize>,
        workspace: Option<&Path>,
        owner_session_id: &str,
    ) -> Vec<TaskSummary> {
        self.list_tasks_visible_to(limit, workspace, Some(owner_session_id))
            .await
    }

    async fn list_tasks_visible_to(
        &self,
        limit: Option<usize>,
        workspace: Option<&Path>,
        owner_session_id: Option<&str>,
    ) -> Vec<TaskSummary> {
        let state = self.state.lock().await;
        let mut items = state
            .tasks
            .values()
            .filter(|record| {
                workspace.is_none_or(|workspace| record.workspace.as_path() == workspace)
                    && owner_session_id.is_none_or(|owner_session_id| {
                        record.owner_session_id.as_deref() == Some(owner_session_id)
                    })
            })
            .map(TaskSummary::from)
            .collect::<Vec<_>>();
        items.sort_by_key(|i| std::cmp::Reverse(i.created_at));
        if let Some(limit) = limit {
            items.truncate(limit);
        }
        items
    }

    /// Retrieve a task by full id or prefix.
    pub async fn get_task(&self, id_or_prefix: &str) -> Result<TaskRecord> {
        self.get_task_visible_to(id_or_prefix, None).await
    }

    /// Retrieve a session-owned task by full id or prefix.
    ///
    /// Ownership is applied before id resolution so foreign records cannot
    /// disclose their existence through exact matches or prefix ambiguity.
    pub async fn get_task_for_owner(
        &self,
        id_or_prefix: &str,
        owner_session_id: &str,
    ) -> Result<TaskRecord> {
        self.get_task_visible_to(id_or_prefix, Some(owner_session_id))
            .await
    }

    /// Retrieve the exact owned task stamped onto a trusted runtime thread.
    ///
    /// The runtime thread supplies a full durable id rather than model input.
    /// Legacy ownerless tasks fail closed even when restored as active.
    pub(crate) async fn get_task_for_active_runtime(&self, task_id: &str) -> Result<TaskRecord> {
        let state = self.state.lock().await;
        state
            .tasks
            .get(task_id)
            .filter(|task| task.owner_session_id.is_some())
            .cloned()
            .ok_or_else(|| anyhow!("Task not found: {task_id}"))
    }

    async fn get_task_visible_to(
        &self,
        id_or_prefix: &str,
        owner_session_id: Option<&str>,
    ) -> Result<TaskRecord> {
        let state = self.state.lock().await;
        let id = resolve_task_id_visible_to(&state.tasks, id_or_prefix, owner_session_id)?;
        state
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("Task not found: {id_or_prefix}"))
    }

    /// Cancel a queued or running task by id/prefix.
    pub async fn cancel_task(&self, id_or_prefix: &str) -> Result<TaskCancellation> {
        self.cancel_task_visible_to(id_or_prefix, None).await
    }

    /// Cancel a queued or running task owned by the given session.
    ///
    /// Ownerless legacy and foreign records fail closed.
    pub async fn cancel_task_for_owner(
        &self,
        id_or_prefix: &str,
        owner_session_id: &str,
    ) -> Result<TaskCancellation> {
        self.cancel_task_visible_to(id_or_prefix, Some(owner_session_id))
            .await
    }

    /// Cancel the exact owned task stamped onto a trusted runtime thread.
    pub(crate) async fn cancel_task_for_active_runtime(
        &self,
        task_id: &str,
    ) -> Result<TaskCancellation> {
        self.get_task_for_active_runtime(task_id).await?;
        self.cancel_task(task_id).await
    }

    async fn cancel_task_visible_to(
        &self,
        id_or_prefix: &str,
        owner_session_id: Option<&str>,
    ) -> Result<TaskCancellation> {
        let mut state = self.state.lock().await;
        let id = resolve_task_id_visible_to(&state.tasks, id_or_prefix, owner_session_id)?;
        let now = Utc::now();

        let mut cancel_running = false;
        let disposition = {
            let task = state
                .tasks
                .get_mut(&id)
                .ok_or_else(|| anyhow!("Task not found: {id}"))?;
            match task.status {
                TaskStatus::Queued => {
                    task.status = TaskStatus::Canceled;
                    task.lifecycle_seq = task.lifecycle_seq.saturating_add(1);
                    task.ended_at = Some(now);
                    task.duration_ms = Some(0);
                    push_timeline_entry(
                        task,
                        TaskTimelineEntry {
                            timestamp: now,
                            kind: "canceled".to_string(),
                            summary: "Task canceled before execution".to_string(),
                            detail_path: None,
                        },
                    );
                    state.queue.retain(|queued_id| queued_id != &id);
                    TaskCancelDisposition::Forced
                }
                TaskStatus::Running => {
                    cancel_running = true;
                    task.lifecycle_seq = task.lifecycle_seq.saturating_add(1);
                    push_timeline_entry(
                        task,
                        TaskTimelineEntry {
                            timestamp: now,
                            kind: "cancel_requested".to_string(),
                            summary: "Cancellation requested".to_string(),
                            detail_path: None,
                        },
                    );
                    TaskCancelDisposition::Requested
                }
                TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Canceled => {
                    TaskCancelDisposition::AlreadyFinished
                }
            }
        };

        if cancel_running && let Some(token) = state.running_cancel.get(&id) {
            token.cancel();
        }

        self.persist_all_locked(&state)?;
        let task = state
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("Task not found: {id}"))?;
        Ok(TaskCancellation { task, disposition })
    }

    /// Return aggregate status counters.
    pub async fn counts(&self) -> TaskCounts {
        let state = self.state.lock().await;
        let mut counts = TaskCounts::default();
        for task in state.tasks.values() {
            match task.status {
                TaskStatus::Queued => counts.queued += 1,
                TaskStatus::Running => counts.running += 1,
                TaskStatus::Completed => counts.completed += 1,
                TaskStatus::Failed => counts.failed += 1,
                TaskStatus::Canceled => counts.canceled += 1,
            }
        }
        counts
    }

    /// Root directory for durable task state.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.cfg.data_dir.clone()
    }

    /// Resolve a task artifact reference to an absolute path.
    #[must_use]
    pub fn artifact_absolute_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cfg.data_dir.join(path)
        }
    }

    /// Write a durable task artifact and return the persisted path reference.
    pub fn write_task_artifact(
        &self,
        task_id: &str,
        label: &str,
        content: &str,
    ) -> Result<PathBuf> {
        self.write_artifact(task_id, label, content)
    }

    /// Apply model-visible tool metadata to a task and persist it.
    pub async fn record_tool_metadata(
        &self,
        id_or_prefix: &str,
        metadata: &Value,
    ) -> Result<TaskRecord> {
        let mut state = self.state.lock().await;
        let id = resolve_task_id(&state.tasks, id_or_prefix)?;
        let updated = {
            let task = state
                .tasks
                .get_mut(&id)
                .ok_or_else(|| anyhow!("Task not found: {id}"))?;
            self.apply_task_update_metadata(task, Some(metadata))?;
            task.clone()
        };
        self.persist_task_locked(&updated)?;
        Ok(updated)
    }

    async fn worker_loop(self: Arc<Self>) {
        loop {
            if self.cancel_token.is_cancelled() {
                tracing::debug!("Worker exiting due to shutdown");
                break;
            }
            let next = {
                let mut state = self.state.lock().await;
                match state.queue.pop_front() {
                    None => None,
                    Some(task_id) => {
                        if let Some(task) = state.tasks.get_mut(&task_id) {
                            if task.status != TaskStatus::Queued {
                                let _ = self.persist_queue_locked(&state.queue);
                                None
                            } else {
                                let now = Utc::now();
                                task.status = TaskStatus::Running;
                                task.lifecycle_seq = task.lifecycle_seq.saturating_add(1);
                                task.started_at = Some(now);
                                task.ended_at = None;
                                task.duration_ms = None;
                                task.error = None;
                                push_timeline_entry(
                                    task,
                                    TaskTimelineEntry {
                                        timestamp: now,
                                        kind: "running".to_string(),
                                        summary: "Task started".to_string(),
                                        detail_path: None,
                                    },
                                );

                                let request = {
                                    ExecutionTask {
                                        id: task.id.clone(),
                                        prompt: task.prompt.clone(),
                                        model: task.model.clone(),
                                        workspace: task.workspace.clone(),
                                        mode_label: task.mode.clone(),
                                        allow_shell: task.allow_shell,
                                        trust_mode: task.trust_mode,
                                        auto_approve: task.auto_approve,
                                    }
                                };
                                let cancel = CancellationToken::new();
                                state.running_cancel.insert(task_id.clone(), cancel.clone());

                                if let Err(err) = self.persist_all_locked(&state) {
                                    tracing::error!("Failed to persist task start: {err}");
                                }
                                Some((task_id, request, cancel))
                            }
                        } else {
                            let _ = self.persist_queue_locked(&state.queue);
                            None
                        }
                    }
                }
            };

            let Some((task_id, request, cancel)) = next else {
                tokio::select! {
                    _ = self.cancel_token.cancelled() => {
                        tracing::debug!("Worker exiting during wait");
                        break;
                    }
                    _ = self.notify.notified() => {}
                }
                continue;
            };

            self.run_task(task_id, request, cancel).await;
        }
    }

    async fn run_task(&self, task_id: String, request: ExecutionTask, cancel: CancellationToken) {
        let (event_tx, mut event_rx) = mpsc::channel(TASK_EVENT_CHANNEL_CAPACITY);
        let exec_fut = self
            .executor
            .execute(request.clone(), event_tx, cancel.clone());
        tokio::pin!(exec_fut);

        let mut guard = ExecutionGuard::new(self.cfg.execution_limits, Instant::now());
        let mut dirty = false;
        let mut accumulated_result_text = String::new();
        let persist_debounce = self.cfg.execution_limits.persist_debounce;

        let (mut result, manager_terminalized) = loop {
            let mut action = guard.evaluate(
                Instant::now(),
                cancel.is_cancelled(),
                self.cancel_token.is_cancelled(),
            );
            if matches!(
                action,
                GuardAction::Interrupt {
                    reason: TaskTerminalReason::IdleTimeout
                }
            ) {
                // Progress already accepted by the executor must win an idle
                // deadline race. Drain only the events queued at this instant
                // so a producer cannot keep the watchdog from re-evaluating
                // wall time, shutdown, or explicit cancellation indefinitely.
                let queued = event_rx.len();
                for _ in 0..queued {
                    let Ok(event) = event_rx.try_recv() else {
                        break;
                    };
                    self.process_execution_event(
                        &task_id,
                        event,
                        &mut guard,
                        &mut accumulated_result_text,
                        &mut dirty,
                    )
                    .await;
                }
                action = guard.evaluate(
                    Instant::now(),
                    cancel.is_cancelled(),
                    self.cancel_token.is_cancelled(),
                );
            }
            match action {
                GuardAction::Interrupt { reason } => {
                    cancel.cancel();
                    guard.note_interrupt(Instant::now(), reason);
                    continue;
                }
                GuardAction::Terminalize { reason } => {
                    break (TaskExecutionResult::from_reason(reason, None), true);
                }
                GuardAction::Run { wait } => {
                    tokio::select! {
                        biased;
                        exec_result = &mut exec_fut => {
                            break (guard.preserve_timeout_reason(exec_result), false);
                        }
                        maybe_event = event_rx.recv() => {
                            if let Some(event) = maybe_event {
                                self.process_execution_event(
                                    &task_id,
                                    event,
                                    &mut guard,
                                    &mut accumulated_result_text,
                                    &mut dirty,
                                )
                                .await;
                            }
                        }
                        _ = self.cancel_token.cancelled(), if !self.cancel_token.is_cancelled() => {
                            cancel.cancel();
                        }
                        _ = sleep(persist_debounce), if dirty => {
                            if let Err(err) = self.flush_task(&task_id).await {
                                tracing::error!("Failed to debounce-persist task {task_id}: {err}");
                            }
                            dirty = false;
                        }
                        _ = sleep(wait) => {}
                    }
                }
            }
        };

        while let Ok(event) = event_rx.try_recv() {
            append_message_delta(&mut accumulated_result_text, &event);
            if let Err(err) = self.apply_execution_event(&task_id, event).await {
                tracing::error!("Failed to apply trailing task event for {task_id}: {err}");
            }
        }
        if manager_terminalized {
            result.result_text = optional_nonzero_text(accumulated_result_text);
        }
        if dirty && let Err(err) = self.flush_task(&task_id).await {
            tracing::error!("Failed to flush task {task_id}: {err}");
        }

        if let Err(err) = self
            .finish_task(&task_id, result, cancel, &request.mode_label)
            .await
        {
            tracing::error!("Failed to finalize task {task_id}: {err}");
        }
    }

    async fn process_execution_event(
        &self,
        task_id: &str,
        event: TaskExecutionEvent,
        guard: &mut ExecutionGuard,
        accumulated_result_text: &mut String,
        dirty: &mut bool,
    ) {
        if execution_event_is_progress(&event) {
            guard.note_progress(Instant::now());
        }
        append_message_delta(accumulated_result_text, &event);
        match self.apply_execution_event(task_id, event).await {
            Ok(outcome) => {
                *dirty = !outcome.persisted;
            }
            Err(err) => {
                tracing::error!("Failed to apply task event for {task_id}: {err}");
            }
        }
    }

    async fn apply_execution_event(
        &self,
        task_id: &str,
        event: TaskExecutionEvent,
    ) -> Result<EventApplyOutcome> {
        let persist_now = execution_event_persist_urgent(&event);
        let mut state = self.state.lock().await;
        let Some(task) = state.tasks.get_mut(task_id) else {
            return Ok(EventApplyOutcome { persisted: true });
        };

        match event {
            TaskExecutionEvent::ThreadLinked { thread_id, turn_id } => {
                task.thread_id = Some(thread_id.clone());
                task.turn_id = Some(turn_id.clone());
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "runtime_link".to_string(),
                        summary: format!("Linked runtime thread {thread_id} turn {turn_id}"),
                        detail_path: None,
                    },
                );
            }
            TaskExecutionEvent::Status { message } => {
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "status".to_string(),
                        summary: summarize_text(&message, TIMELINE_SUMMARY_LIMIT),
                        detail_path: None,
                    },
                );
            }
            TaskExecutionEvent::MessageDelta { content } => {
                if !content.trim().is_empty() {
                    push_timeline_entry(
                        task,
                        TaskTimelineEntry {
                            timestamp: Utc::now(),
                            kind: "message".to_string(),
                            summary: summarize_text(&content, TIMELINE_SUMMARY_LIMIT),
                            detail_path: None,
                        },
                    );
                }
            }
            TaskExecutionEvent::ToolStarted { id, name, input } => {
                let input_summary = summarize_json(&input);
                task.tool_calls.push(TaskToolCallSummary {
                    id: id.clone(),
                    name: name.clone(),
                    status: TaskToolStatus::Running,
                    started_at: Utc::now(),
                    ended_at: None,
                    duration_ms: None,
                    input_summary: input_summary.clone(),
                    output_summary: None,
                    detail_path: None,
                    patch_ref: None,
                });
                let summary = input_summary
                    .map(|s| format!("{name} started ({s})"))
                    .unwrap_or_else(|| format!("{name} started"));
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "tool_started".to_string(),
                        summary,
                        detail_path: None,
                    },
                );
            }
            TaskExecutionEvent::ToolProgress { id, output } => {
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "tool_progress".to_string(),
                        summary: format!(
                            "{id}: {}",
                            summarize_text(&output, TIMELINE_SUMMARY_LIMIT.saturating_sub(8))
                        ),
                        detail_path: None,
                    },
                );
            }
            TaskExecutionEvent::ToolCompleted {
                id,
                name,
                success,
                output,
                metadata,
            } => {
                let now = Utc::now();
                let detail_path = self.artifact_if_large(task_id, &name, &output)?;
                let output_summary = summarize_text(&output, TIMELINE_SUMMARY_LIMIT);
                let patch_ref = if name == "apply_patch" {
                    detail_path.clone()
                } else {
                    None
                };

                if let Some(call) = task.tool_calls.iter_mut().find(|call| call.id == id) {
                    call.status = if success {
                        TaskToolStatus::Success
                    } else {
                        TaskToolStatus::Failed
                    };
                    call.ended_at = Some(now);
                    call.duration_ms = Some(duration_ms(call.started_at, now));
                    call.output_summary = Some(output_summary.clone());
                    call.detail_path = detail_path.clone();
                    call.patch_ref = patch_ref.clone();

                    if call.duration_ms.is_none()
                        && let Some(duration) = metadata
                            .as_ref()
                            .and_then(|m| m.get("duration_ms"))
                            .and_then(Value::as_u64)
                    {
                        call.duration_ms = Some(duration);
                    }
                }

                let status = if success { "success" } else { "failed" };
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: now,
                        kind: "tool_completed".to_string(),
                        summary: format!("{name} {status}: {output_summary}"),
                        detail_path: detail_path.clone(),
                    },
                );
                if let Some(patch_ref) = patch_ref {
                    push_timeline_entry(
                        task,
                        TaskTimelineEntry {
                            timestamp: now,
                            kind: "patch_ref".to_string(),
                            summary: format!("Patch artifact: {}", patch_ref.display()),
                            detail_path: Some(patch_ref),
                        },
                    );
                }

                self.apply_task_update_metadata(task, metadata.as_ref())?;
            }
            TaskExecutionEvent::Error { message } => {
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "error".to_string(),
                        summary: summarize_text(&message, TIMELINE_SUMMARY_LIMIT),
                        detail_path: None,
                    },
                );
            }
            TaskExecutionEvent::RuntimeEvent {
                seq,
                event,
                summary,
            } => {
                task.runtime_event_count = task.runtime_event_count.saturating_add(1);
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "runtime_event".to_string(),
                        summary: format!("#{seq} {event}: {summary}"),
                        detail_path: None,
                    },
                );
            }
        }

        if persist_now {
            self.persist_task_locked(task)?;
        }
        Ok(EventApplyOutcome {
            persisted: persist_now,
        })
    }

    async fn flush_task(&self, task_id: &str) -> Result<()> {
        let state = self.state.lock().await;
        if let Some(task) = state.tasks.get(task_id) {
            self.persist_task_locked(task)?;
        }
        Ok(())
    }

    async fn finish_task(
        &self,
        task_id: &str,
        mut result: TaskExecutionResult,
        cancel: CancellationToken,
        mode_label: &str,
    ) -> Result<()> {
        let mut state = self.state.lock().await;
        state.running_cancel.remove(task_id);
        let Some(task) = state.tasks.get_mut(task_id) else {
            return Ok(());
        };

        let now = Utc::now();
        if cancel.is_cancelled() && result.status == TaskStatus::Completed {
            result.status = TaskStatus::Canceled;
            result.result_text = None;
            result.error = None;
            result.terminal_reason = TaskTerminalReason::Canceled;
        }
        if self.cancel_token.is_cancelled()
            && result.status != TaskStatus::Completed
            && matches!(
                result.terminal_reason,
                TaskTerminalReason::Canceled | TaskTerminalReason::Failed
            )
        {
            result.status = TaskStatus::Canceled;
            result.terminal_reason = TaskTerminalReason::Shutdown;
            result.error = Some(TaskTerminalReason::Shutdown.receipt_message());
        }

        task.status = result.status;
        task.lifecycle_seq = task.lifecycle_seq.saturating_add(1);
        task.mode = mode_label.to_string();
        task.ended_at = Some(now);
        task.duration_ms = task.started_at.map(|start| duration_ms(start, now));
        task.error = result.error.clone();
        task.terminal_reason = Some(result.terminal_reason.as_str().to_string());
        let finished_summary = if matches!(result.status, TaskStatus::Queued | TaskStatus::Running)
        {
            format!("Task ended in unexpected state: {mode_label}")
        } else {
            match result.terminal_reason {
                TaskTerminalReason::Completed
                | TaskTerminalReason::Canceled
                | TaskTerminalReason::Shutdown => result.terminal_reason.receipt_message(),
                TaskTerminalReason::Failed
                | TaskTerminalReason::WallTimeout
                | TaskTerminalReason::IdleTimeout
                | TaskTerminalReason::CancelTimeout => format!(
                    "{}: {}",
                    result.terminal_reason.as_str(),
                    result
                        .error
                        .as_deref()
                        .map(|e| summarize_text(e, TIMELINE_SUMMARY_LIMIT))
                        .unwrap_or_else(|| result.terminal_reason.receipt_message())
                ),
            }
        };
        push_timeline_entry(
            task,
            TaskTimelineEntry {
                timestamp: now,
                kind: "finished".to_string(),
                summary: finished_summary,
                detail_path: None,
            },
        );

        if let Some(text) = result.result_text {
            let detail_path = self.artifact_if_large(task_id, "result", &text)?;
            task.result_summary = Some(summarize_text(&text, TIMELINE_SUMMARY_LIMIT));
            task.result_detail_path = detail_path.clone();
            if let Some(detail_path) = detail_path {
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: now,
                        kind: "result_ref".to_string(),
                        summary: format!("Result artifact: {}", detail_path.display()),
                        detail_path: Some(detail_path),
                    },
                );
            }
        } else if result.status == TaskStatus::Completed {
            task.result_summary = Some("(no textual output)".to_string());
        }

        self.persist_all_locked(&state)?;
        Ok(())
    }

    fn artifact_if_large(
        &self,
        task_id: &str,
        label: &str,
        content: &str,
    ) -> Result<Option<PathBuf>> {
        if content.len() < ARTIFACT_THRESHOLD {
            return Ok(None);
        }
        self.write_artifact(task_id, label, content).map(Some)
    }

    fn write_artifact(&self, task_id: &str, label: &str, content: &str) -> Result<PathBuf> {
        ensure_safe_storage_id("task id", task_id)?;
        let artifact_dir = self.artifacts_dir.join(task_id);
        fs::create_dir_all(&artifact_dir)
            .with_context(|| format!("Failed to create artifact dir {}", artifact_dir.display()))?;
        let stamp = Utc::now().format("%Y%m%dT%H%M%S%.3fZ");
        let filename = format!("{stamp}_{}.txt", sanitize_filename(label));
        let absolute = artifact_dir.join(filename);
        fs::write(&absolute, content)
            .with_context(|| format!("Failed to write artifact {}", absolute.display()))?;
        let relative = absolute
            .strip_prefix(&self.cfg.data_dir)
            .map(PathBuf::from)
            .unwrap_or(absolute);
        Ok(relative)
    }

    fn apply_task_update_metadata(
        &self,
        task: &mut TaskRecord,
        metadata: Option<&Value>,
    ) -> Result<()> {
        let Some(updates) = metadata.and_then(|m| m.get("task_updates")) else {
            return Ok(());
        };
        let now = Utc::now();

        if let Some(value) = updates.get("checklist") {
            let mut checklist: TaskChecklistState = serde_json::from_value(value.clone())
                .context("Failed to parse checklist task update")?;
            checklist.updated_at = checklist.updated_at.or(Some(now));
            task.checklist = checklist;
            push_timeline_entry(
                task,
                TaskTimelineEntry {
                    timestamp: now,
                    kind: "checklist".to_string(),
                    summary: format!(
                        "Checklist updated: {} item(s), {}% complete",
                        task.checklist.items.len(),
                        task.checklist.completion_pct
                    ),
                    detail_path: None,
                },
            );
        }

        if let Some(value) = updates.get("gate") {
            let gate: TaskGateRecord = serde_json::from_value(value.clone())
                .context("Failed to parse gate task update")?;
            let summary = format!("Gate {} {}: {}", gate.gate, gate.status, gate.summary);
            task.gates.retain(|existing| existing.id != gate.id);
            task.gates.push(gate.clone());
            push_timeline_entry(
                task,
                TaskTimelineEntry {
                    timestamp: now,
                    kind: "gate".to_string(),
                    summary: summarize_text(&summary, TIMELINE_SUMMARY_LIMIT),
                    detail_path: gate.log_path,
                },
            );
        }

        if let Some(value) = updates.get("hunt_verdict") {
            let raw = value
                .as_str()
                .ok_or_else(|| anyhow!("hunt_verdict task update must be a string"))?;
            let verdict = normalize_hunt_verdict(raw)?;
            if task.hunt_verdict.as_deref() != Some(verdict) {
                task.hunt_verdict = Some(verdict.to_string());
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: now,
                        kind: "hunt_verdict".to_string(),
                        summary: format!("Hunt verdict updated: {verdict}"),
                        detail_path: None,
                    },
                );
            }
        }

        if let Some(value) = updates.get("attempt") {
            let attempt: TaskAttemptRecord = serde_json::from_value(value.clone())
                .context("Failed to parse attempt task update")?;
            task.attempts.retain(|existing| existing.id != attempt.id);
            task.attempts.push(attempt.clone());
            push_timeline_entry(
                task,
                TaskTimelineEntry {
                    timestamp: now,
                    kind: "pr_attempt".to_string(),
                    summary: format!(
                        "Attempt {}/{} recorded for {}",
                        attempt.attempt_index, attempt.attempt_count, attempt.attempt_group_id
                    ),
                    detail_path: attempt.patch_path,
                },
            );
        }

        if let Some(value) = updates.get("artifacts")
            && let Some(items) = value.as_array()
        {
            for item in items {
                let artifact: TaskArtifactRef = serde_json::from_value(item.clone())
                    .context("Failed to parse artifact task update")?;
                push_timeline_entry(
                    task,
                    TaskTimelineEntry {
                        timestamp: now,
                        kind: "artifact".to_string(),
                        summary: format!("{}: {}", artifact.label, artifact.summary),
                        detail_path: Some(artifact.path.clone()),
                    },
                );
                task.artifacts.push(artifact);
            }
        }

        if let Some(value) = updates.get("github_event") {
            let event: TaskGithubEvent = serde_json::from_value(value.clone())
                .context("Failed to parse GitHub task update")?;
            push_timeline_entry(
                task,
                TaskTimelineEntry {
                    timestamp: now,
                    kind: "github".to_string(),
                    summary: format!(
                        "{} {}#{}: {}",
                        event.action, event.target, event.number, event.summary
                    ),
                    detail_path: None,
                },
            );
            task.github_events.push(event);
        }

        Ok(())
    }

    fn persist_all_locked(&self, state: &ManagerState) -> Result<()> {
        self.persist_queue_locked(&state.queue)?;
        for task in state.tasks.values() {
            self.persist_task_locked(task)?;
        }
        Ok(())
    }

    fn persist_queue_locked(&self, queue: &VecDeque<String>) -> Result<()> {
        write_json_atomic(
            &self.queue_path,
            &QueueFile {
                queue: queue.iter().cloned().collect(),
            },
        )
    }

    fn persist_task_locked(&self, task: &TaskRecord) -> Result<()> {
        let path = self.tasks_dir.join(format!("{}.json", task.id));
        write_json_atomic(&path, task)
    }
}

fn normalize_hunt_verdict(raw: &str) -> Result<&'static str> {
    match raw.trim() {
        "hunting" => Ok("hunting"),
        "hunted" => Ok("hunted"),
        "wounded" => Ok("wounded"),
        "escaped" => Ok("escaped"),
        other => bail!(
            "unsupported hunt_verdict task update '{other}'. Expected one of: hunting, hunted, wounded, escaped"
        ),
    }
}

/// Outcome of loading the persisted task store at boot: the reconciled task
/// map + queue, plus the ids whose status was flipped running->failed by
/// crash recovery (the only records boot needs to re-persist).
struct LoadedTaskState {
    tasks: HashMap<String, TaskRecord>,
    queue: VecDeque<String>,
    recovered: Vec<String>,
}

fn load_state(tasks_dir: &Path, queue_path: &Path) -> Result<LoadedTaskState> {
    let mut tasks = HashMap::new();
    let mut recovered = Vec::new();
    if tasks_dir.exists() {
        for entry in fs::read_dir(tasks_dir)
            .with_context(|| format!("Failed to read tasks dir {}", tasks_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read task file {}", path.display()))?;
            let mut task: TaskRecord = serde_json::from_str(&content)
                .with_context(|| format!("Failed to parse task file {}", path.display()))?;
            if task.schema_version > CURRENT_TASK_SCHEMA_VERSION {
                bail!(
                    "Task schema v{} is newer than supported v{}",
                    task.schema_version,
                    CURRENT_TASK_SCHEMA_VERSION
                );
            }
            if task.status == TaskStatus::Running {
                let now = Utc::now();
                let duration_ms = task.started_at.and_then(|started| {
                    u64::try_from(now.signed_duration_since(started).num_milliseconds()).ok()
                });
                task.status = TaskStatus::Failed;
                task.lifecycle_seq = task.lifecycle_seq.saturating_add(1);
                task.ended_at = Some(now);
                task.duration_ms = duration_ms;
                task.terminal_reason = Some(TaskTerminalReason::Failed.as_str().to_string());
                task.error = Some(
                    "Interrupted by process restart; prior process is not attached".to_string(),
                );
                for tool in &mut task.tool_calls {
                    if tool.status == TaskToolStatus::Running {
                        tool.status = TaskToolStatus::Failed;
                        tool.ended_at = Some(now);
                        tool.duration_ms = duration_ms.or_else(|| {
                            u64::try_from(
                                now.signed_duration_since(tool.started_at)
                                    .num_milliseconds(),
                            )
                            .ok()
                        });
                    }
                }
                push_timeline_entry(
                    &mut task,
                    TaskTimelineEntry {
                        timestamp: now,
                        kind: "recovered".to_string(),
                        summary: "Interrupted by process restart; prior process is not attached"
                            .to_string(),
                        detail_path: None,
                    },
                );
                recovered.push(task.id.clone());
            }
            tasks.insert(task.id.clone(), task);
        }
    }

    let mut queue = if queue_path.exists() {
        let content = fs::read_to_string(queue_path)
            .with_context(|| format!("Failed to read queue file {}", queue_path.display()))?;
        let parsed: QueueFile = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse queue file {}", queue_path.display()))?;
        VecDeque::from(parsed.queue)
    } else {
        VecDeque::new()
    };

    queue.retain(|id| {
        tasks
            .get(id)
            .is_some_and(|task| task.status == TaskStatus::Queued)
    });

    let known = queue.iter().cloned().collect::<HashSet<_>>();
    let mut missing = tasks
        .values()
        .filter(|task| task.status == TaskStatus::Queued && !known.contains(&task.id))
        .map(|task| task.id.clone())
        .collect::<Vec<_>>();
    missing.sort();
    for id in missing {
        queue.push_back(id);
    }

    Ok(LoadedTaskState {
        tasks,
        queue,
        recovered,
    })
}

struct EventApplyOutcome {
    persisted: bool,
}

fn execution_event_is_progress(event: &TaskExecutionEvent) -> bool {
    matches!(
        event,
        TaskExecutionEvent::MessageDelta { .. }
            | TaskExecutionEvent::ToolStarted { .. }
            | TaskExecutionEvent::ToolProgress { .. }
            | TaskExecutionEvent::ToolCompleted { .. }
    )
}

fn execution_event_persist_urgent(event: &TaskExecutionEvent) -> bool {
    !matches!(
        event,
        TaskExecutionEvent::MessageDelta { .. }
            | TaskExecutionEvent::ToolProgress { .. }
            | TaskExecutionEvent::RuntimeEvent { .. }
    )
}

fn timeline_kinds_coalesce(left: &str, right: &str) -> bool {
    matches!(
        (left, right),
        ("message", "message")
            | ("tool_progress", "tool_progress")
            | ("runtime_event", "runtime_event")
    )
}

fn push_timeline_entry(task: &mut TaskRecord, entry: TaskTimelineEntry) {
    if let Some(last) = task.timeline.last_mut()
        && timeline_kinds_coalesce(last.kind.as_str(), entry.kind.as_str())
    {
        last.timestamp = entry.timestamp;
        last.summary = entry.summary;
        last.detail_path = entry.detail_path;
        return;
    }
    task.timeline.push(entry);
    trim_task_timeline(&mut task.timeline);
}

fn trim_task_timeline(entries: &mut Vec<TaskTimelineEntry>) {
    if entries.len() <= TIMELINE_ENTRY_LIMIT {
        return;
    }
    let overflow = entries.len() - TIMELINE_ENTRY_LIMIT;
    let start = TIMELINE_HEAD_KEEP.min(entries.len().saturating_sub(overflow + 1));
    let end = start + overflow;
    if start >= end || end > entries.len() {
        entries.truncate(TIMELINE_ENTRY_LIMIT);
        return;
    }
    entries.drain(start..end);
    let omitted = TaskTimelineEntry {
        timestamp: Utc::now(),
        kind: "omitted".to_string(),
        summary: format!("{overflow} earlier events omitted to bound storage"),
        detail_path: None,
    };
    if entries
        .get(start)
        .is_none_or(|entry| entry.kind != "omitted")
    {
        entries.insert(start, omitted);
    }
    if entries.len() > TIMELINE_ENTRY_LIMIT {
        let extra = entries.len() - TIMELINE_ENTRY_LIMIT;
        let drop_at = (start + 1).min(entries.len().saturating_sub(1));
        let drop_end = (drop_at + extra).min(entries.len());
        if drop_at < drop_end {
            entries.drain(drop_at..drop_end);
        } else {
            entries.truncate(TIMELINE_ENTRY_LIMIT);
        }
    }
}

fn resolve_task_id_visible_to(
    tasks: &HashMap<String, TaskRecord>,
    id_or_prefix: &str,
    owner_session_id: Option<&str>,
) -> Result<String> {
    let visible = |record: &TaskRecord| {
        owner_session_id.is_none_or(|owner_session_id| {
            record.owner_session_id.as_deref() == Some(owner_session_id)
        })
    };
    if tasks.get(id_or_prefix).is_some_and(visible) {
        return Ok(id_or_prefix.to_string());
    }
    let matches = tasks
        .iter()
        .filter(|(id, record)| id.starts_with(id_or_prefix) && visible(record))
        .map(|(id, _)| id)
        .cloned()
        .collect::<Vec<_>>();
    match matches.len() {
        0 => bail!("Task not found: {id_or_prefix}"),
        1 => Ok(matches[0].clone()),
        _ => bail!(
            "Ambiguous task prefix '{}': matches {} tasks",
            id_or_prefix,
            matches.len()
        ),
    }
}

fn resolve_task_id(tasks: &HashMap<String, TaskRecord>, id_or_prefix: &str) -> Result<String> {
    resolve_task_id_visible_to(tasks, id_or_prefix, None)
}

fn summarize_json(value: &Value) -> Option<String> {
    let text = serde_json::to_string(value).ok()?;
    Some(summarize_text(&text, TIMELINE_SUMMARY_LIMIT))
}

fn summarize_text(text: &str, limit: usize) -> String {
    let take = limit.saturating_sub(3);
    let mut count = 0;
    let mut out = String::new();
    for ch in text.chars() {
        if count >= take {
            out.push_str("...");
            return out;
        }
        if ch.is_control() && ch != '\n' && ch != '\t' {
            continue;
        }
        out.push(ch);
        count += 1;
    }
    out
}

fn ensure_safe_storage_id(kind: &str, value: &str) -> Result<()> {
    let mut components = Path::new(value).components();
    let Some(component) = components.next() else {
        bail!("{kind} must not be empty");
    };
    if components.next().is_some() || !matches!(component, std::path::Component::Normal(_)) {
        bail!("{kind} must be a single path component");
    }
    Ok(())
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

fn duration_ms(start: DateTime<Utc>, end: DateTime<Utc>) -> u64 {
    let millis = (end - start).num_milliseconds();
    if millis.is_negative() {
        0
    } else {
        u64::try_from(millis).unwrap_or(u64::MAX)
    }
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let payload = serde_json::to_string_pretty(value)?;
    crate::utils::write_atomic(path, payload.as_bytes())
        .with_context(|| format!("Failed to write {}", path.display()))
}

fn default_auto_approve() -> bool {
    true
}

/// Default task manager data location (`~/.codewhale/tasks`, or legacy
/// `~/.deepseek/tasks` when only the legacy directory exists).
#[must_use]
pub fn default_tasks_dir() -> PathBuf {
    for var in ["CODEWHALE_TASKS_DIR", "DEEPSEEK_TASKS_DIR"] {
        if let Ok(path) = std::env::var(var)
            && !path.trim().is_empty()
        {
            return PathBuf::from(path);
        }
    }
    if let Some(home) = codewhale_paths::codewhale_home_override().ok().flatten() {
        return home.join("tasks");
    }
    codewhale_paths::user_home()
        .map(|home| default_tasks_dir_for_home(&home))
        .unwrap_or_else(|| PathBuf::from(".codewhale").join("tasks"))
}

fn default_tasks_dir_for_home(home: &Path) -> PathBuf {
    let primary = home.join(".codewhale").join("tasks");
    if primary.is_dir() {
        return primary;
    }
    let legacy = home.join(".deepseek").join("tasks");
    if legacy.is_dir() {
        return legacy;
    }
    primary
}

/// Wait for a task to reach a terminal status (tests and API helpers).
#[cfg(test)]
pub async fn wait_for_terminal_state(
    manager: &TaskManager,
    task_id: &str,
    timeout: StdDuration,
) -> Result<TaskRecord> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let task = manager.get_task(task_id).await?;
        if task.status.is_terminal() {
            return Ok(task);
        }
        if std::time::Instant::now() >= deadline {
            bail!("Timed out waiting for task {task_id}");
        }
        sleep(StdDuration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvVarGuard, lock_test_env};
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::Duration;

    struct MockExecutor;

    fn provider_default_model_cases() -> Vec<(&'static str, Config, &'static str)> {
        let deepseek = Config {
            provider: Some("deepseek".to_string()),
            default_text_model: Some("deepseek-v4-flash".to_string()),
            ..Config::default()
        };

        let zai = Config {
            provider: Some("zai".to_string()),
            // Exercise provider-aware rejection of a stale DeepSeek root default.
            default_text_model: Some(crate::config::DEFAULT_TEXT_MODEL.to_string()),
            ..Config::default()
        };

        let mut custom_providers = crate::config::ProvidersConfig::default();
        custom_providers.custom.insert(
            "acme".to_string(),
            crate::config::ProviderConfig {
                base_url: Some("http://127.0.0.1:1/v1".to_string()),
                model: Some("acme-coder".to_string()),
                kind: Some("openai-compatible".to_string()),
                ..crate::config::ProviderConfig::default()
            },
        );
        let custom = Config {
            provider: Some("acme".to_string()),
            providers: Some(custom_providers),
            ..Config::default()
        };

        vec![
            ("deepseek", deepseek, "deepseek-v4-flash"),
            ("zai", zai, crate::config::DEFAULT_ZAI_MODEL),
            ("custom", custom, "acme-coder"),
        ]
    }

    #[test]
    fn task_manager_config_uses_the_active_provider_default() {
        for (label, config, expected) in provider_default_model_cases() {
            let task_config =
                TaskManagerConfig::from_runtime(&config, PathBuf::from("."), None, Some(1));
            assert_eq!(
                task_config.default_model, expected,
                "{label} durable task default"
            );
        }
    }

    #[async_trait]
    impl TaskExecutor for MockExecutor {
        async fn execute(
            &self,
            task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            let _ = events
                .send(TaskExecutionEvent::Status {
                    message: format!("running {}", task.id),
                })
                .await;
            let _ = events
                .send(TaskExecutionEvent::ThreadLinked {
                    thread_id: "thr_test".to_string(),
                    turn_id: "turn_test".to_string(),
                })
                .await;
            let _ = events
                .send(TaskExecutionEvent::ToolStarted {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "README.md" }),
                })
                .await;
            sleep(Duration::from_millis(50)).await;
            if cancel.is_cancelled() {
                return TaskExecutionResult {
                    status: TaskStatus::Canceled,
                    result_text: None,
                    error: None,
                    terminal_reason: TaskTerminalReason::Canceled,
                };
            }
            let _ = events
                .send(TaskExecutionEvent::ToolCompleted {
                    id: "tool_1".to_string(),
                    name: "read_file".to_string(),
                    success: true,
                    output: "read ok".to_string(),
                    metadata: Some(serde_json::json!({
                        "duration_ms": 10,
                        "task_updates": {
                            "checklist": {
                                "items": [
                                    { "id": 1, "content": "read fixture", "status": "in_progress" }
                                ],
                                "completion_pct": 0,
                                "in_progress_id": 1,
                                "updated_at": null
                            }
                        }
                    })),
                })
                .await;
            TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("done".to_string()),
                error: None,
                terminal_reason: TaskTerminalReason::Completed,
            }
        }
    }

    fn test_config(root: PathBuf) -> TaskManagerConfig {
        TaskManagerConfig {
            data_dir: root,
            worker_count: 1,
            default_workspace: PathBuf::from("."),
            default_model: "deepseek-v4-flash".to_string(),
            default_mode: "agent".to_string(),
            allow_shell: false,
            trust_mode: false,
            execution_limits: TaskExecutionLimits::default(),
        }
    }

    fn short_test_config(root: PathBuf) -> TaskManagerConfig {
        TaskManagerConfig {
            execution_limits: TaskExecutionLimits::short_for_tests(),
            ..test_config(root)
        }
    }

    fn wall_timeout_test_config(root: PathBuf) -> TaskManagerConfig {
        let mut config = short_test_config(root);
        config.execution_limits.idle_progress = config
            .execution_limits
            .wall_time
            .saturating_add(config.execution_limits.cancel_grace);
        config
    }

    #[tokio::test]
    async fn persists_and_recovers_task_records() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;

        let task = manager
            .add_task(NewTaskRequest {
                owner_session_id: Some("session-persist".to_string()),
                ..NewTaskRequest::from_prompt("test persistence")
            })
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(finished.thread_id.as_deref(), Some("thr_test"));
        assert_eq!(finished.turn_id.as_deref(), Some("turn_test"));
        assert_eq!(finished.checklist.items.len(), 1);
        assert_eq!(finished.checklist.in_progress_id, Some(1));
        assert!(
            finished.lifecycle_seq >= 3,
            "queued, running, and terminal owner transitions must advance the sequence"
        );

        drop(manager);

        let recovered =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;
        let loaded = recovered.get_task(&task.id).await?;
        assert_eq!(loaded.status, TaskStatus::Completed);
        assert_eq!(
            loaded.owner_session_id.as_deref(),
            Some("session-persist"),
            "session ownership should survive persistence and restart"
        );
        assert!(!loaded.timeline.is_empty());
        assert_eq!(loaded.checklist.items[0].content, "read fixture");
        Ok(())
    }

    #[tokio::test]
    async fn preallocated_task_ids_are_validated_and_collision_safe() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;
        let request = NewTaskRequest::from_prompt("preallocated owner identity");

        let invalid = manager
            .add_task_with_id(request.clone(), "task_short".to_string())
            .await
            .expect_err("invalid preallocated id");
        assert!(invalid.to_string().contains("task_<16hex>"), "{invalid:#}");

        let id = "task_0123456789abcdef".to_string();
        let created = manager
            .add_task_with_id(request.clone(), id.clone())
            .await?;
        assert_eq!(created.id, id);
        assert_eq!(
            created.schema_version, 2,
            "the additive lifecycle field must remain rollback-readable"
        );
        assert_eq!(created.lifecycle_seq, 1);
        let collision = manager
            .add_task_with_id(request, id)
            .await
            .expect_err("task id collision");
        assert!(
            collision.to_string().contains("already exists"),
            "{collision:#}"
        );
        assert_eq!(manager.list_tasks(None).await.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn failed_queue_write_leaves_no_replayable_task_record() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;
        std::fs::remove_file(root.join("queue.json"))?;
        std::fs::create_dir(root.join("queue.json"))?;

        let id = "task_fedcba9876543210".to_string();
        let error = manager
            .add_task_with_id(
                NewTaskRequest::from_prompt("must not resurrect"),
                id.clone(),
            )
            .await
            .expect_err("queue path directory must reject the atomic queue write");
        assert!(error.to_string().contains("queue.json"), "{error:#}");
        assert!(manager.list_tasks(None).await.is_empty());
        assert!(!root.join("tasks").join(format!("{id}.json")).exists());
        assert!(
            !root
                .join("tasks")
                .join(format!(".{id}.json.pending"))
                .exists(),
            "a failed queue write may leave no replayable or staged task record"
        );
        Ok(())
    }

    #[tokio::test]
    async fn list_tasks_scopes_results_to_workspace_before_limit() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        manager
            .add_task(NewTaskRequest {
                prompt: "task in workspace a".to_string(),
                workspace: Some(PathBuf::from("/tmp/workspace-a")),
                ..NewTaskRequest::from_prompt("task in workspace a")
            })
            .await?;
        manager
            .add_task(NewTaskRequest {
                prompt: "task in workspace b".to_string(),
                workspace: Some(PathBuf::from("/tmp/workspace-b")),
                ..NewTaskRequest::from_prompt("task in workspace b")
            })
            .await?;

        let scoped = manager
            .list_tasks_scoped(Some(1), Some(Path::new("/tmp/workspace-a")))
            .await;
        assert_eq!(scoped.len(), 1);
        assert_eq!(scoped[0].workspace, PathBuf::from("/tmp/workspace-a"));
        Ok(())
    }

    #[tokio::test]
    async fn task_controls_are_session_owned_and_legacy_records_fail_closed() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        let mut session_a = sample_task_record();
        session_a.id = "task_dead000000000001".to_string();
        session_a.owner_session_id = Some("session-a".to_string());
        session_a.status = TaskStatus::Completed;
        session_a.created_at = Utc::now() - chrono::Duration::seconds(3);

        let mut session_b = sample_task_record();
        session_b.id = "task_dead000000000002".to_string();
        session_b.owner_session_id = Some("session-b".to_string());
        session_b.status = TaskStatus::Completed;
        session_b.created_at = Utc::now() - chrono::Duration::seconds(2);

        let mut session_b_newest = sample_task_record();
        session_b_newest.id = "task_beef000000000002".to_string();
        session_b_newest.owner_session_id = Some("session-b".to_string());
        session_b_newest.status = TaskStatus::Completed;
        session_b_newest.created_at = Utc::now();

        let mut legacy = sample_task_record();
        legacy.id = "task_dead000000000003".to_string();
        legacy.owner_session_id = None;
        legacy.status = TaskStatus::Completed;
        legacy.created_at = Utc::now() - chrono::Duration::seconds(1);

        {
            let mut state = manager.state.lock().await;
            for record in [
                session_a.clone(),
                session_b.clone(),
                session_b_newest.clone(),
                legacy.clone(),
            ] {
                state.tasks.insert(record.id.clone(), record);
            }
        }

        let session_b_list = manager
            .list_tasks_for_owner(Some(1), None, "session-b")
            .await;
        assert_eq!(session_b_list.len(), 1);
        assert_eq!(session_b_list[0].id, session_b_newest.id);

        let session_b_prefix = manager.get_task_for_owner("task_dead", "session-b").await?;
        assert_eq!(session_b_prefix.id, session_b.id);
        assert!(
            manager
                .get_task_for_owner(&session_a.id, "session-b")
                .await
                .unwrap_err()
                .to_string()
                .contains("Task not found")
        );
        assert!(
            manager
                .get_task_for_owner(&legacy.id, "session-b")
                .await
                .unwrap_err()
                .to_string()
                .contains("Task not found")
        );
        assert!(
            manager
                .get_task_for_active_runtime(&legacy.id)
                .await
                .unwrap_err()
                .to_string()
                .contains("Task not found"),
            "legacy ownerless active tasks must fail closed"
        );
        assert_eq!(
            manager.get_task_for_active_runtime(&session_a.id).await?.id,
            session_a.id
        );

        assert!(
            manager
                .cancel_task_for_owner(&session_a.id, "session-b")
                .await
                .unwrap_err()
                .to_string()
                .contains("Task not found")
        );
        assert_eq!(
            manager.get_task(&session_a.id).await?.status,
            TaskStatus::Completed
        );
        assert!(
            manager
                .cancel_task_for_owner(&legacy.id, "session-b")
                .await
                .unwrap_err()
                .to_string()
                .contains("Task not found")
        );

        let own = manager
            .cancel_task_for_owner(&session_b.id, "session-b")
            .await?;
        assert_eq!(own.disposition, TaskCancelDisposition::AlreadyFinished);
        let active_own = manager
            .cancel_task_for_active_runtime(&session_a.id)
            .await?;
        assert_eq!(
            active_own.disposition,
            TaskCancelDisposition::AlreadyFinished
        );
        assert_eq!(
            manager
                .get_task_for_owner(&session_a.id, "session-a")
                .await?
                .id,
            session_a.id,
            "switching A to B and back must restore A's controls"
        );
        Ok(())
    }

    #[tokio::test]
    async fn boot_does_not_rewrite_non_recovered_task_files() -> Result<()> {
        // #3757 boot-persist narrowing: TaskManager::start must persist only
        // the reconciled queue and the running->failed recoveries — a
        // completed task's file must be byte-identical across a restart.
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("finish then persist"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Completed);
        drop(manager);

        let task_file = root.join("tasks").join(format!("{}.json", task.id));
        let before = fs::read(&task_file)?;

        let recovered =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;
        // Give start() a beat to run its (narrowed) boot persist.
        sleep(Duration::from_millis(50)).await;
        drop(recovered);

        let after = fs::read(&task_file)?;
        assert_eq!(
            before, after,
            "a completed task file must not be rewritten on boot"
        );
        Ok(())
    }

    #[test]
    fn running_tasks_are_not_requeued_after_restart() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let tasks_dir = root.join("tasks");
        fs::create_dir_all(&tasks_dir)?;
        let queue_path = root.join("queue.json");
        let task_id = "task_stale_running".to_string();
        let started_at = Utc::now() - chrono::Duration::seconds(30);
        let task = TaskRecord {
            schema_version: CURRENT_TASK_SCHEMA_VERSION,
            id: task_id.clone(),
            prompt: "long-running shell work".to_string(),
            model: "deepseek-v4-flash".to_string(),
            workspace: PathBuf::from("."),
            mode: "agent".to_string(),
            allow_shell: true,
            trust_mode: false,
            auto_approve: false,
            status: TaskStatus::Running,
            created_at: started_at,
            started_at: Some(started_at),
            ended_at: None,
            duration_ms: None,
            hunt_verdict: None,
            result_summary: None,
            result_detail_path: None,
            error: None,
            terminal_reason: None,
            thread_id: Some("thr_stale".to_string()),
            turn_id: Some("turn_stale".to_string()),
            owner_session_id: Some("session-old".to_string()),
            runtime_event_count: 0,
            lifecycle_seq: 2,
            checklist: TaskChecklistState::default(),
            gates: Vec::new(),
            attempts: Vec::new(),
            artifacts: Vec::new(),
            github_events: Vec::new(),
            tool_calls: vec![TaskToolCallSummary {
                id: "tool_shell".to_string(),
                name: "task_shell_start".to_string(),
                status: TaskToolStatus::Running,
                started_at,
                ended_at: None,
                duration_ms: None,
                input_summary: Some("shell: sleep 999".to_string()),
                output_summary: None,
                detail_path: None,
                patch_ref: None,
            }],
            timeline: vec![TaskTimelineEntry {
                timestamp: started_at,
                kind: "running".to_string(),
                summary: "Task started".to_string(),
                detail_path: None,
            }],
        };
        fs::write(
            tasks_dir.join(format!("{task_id}.json")),
            serde_json::to_string_pretty(&task)?,
        )?;
        fs::write(
            &queue_path,
            serde_json::to_string_pretty(&QueueFile {
                queue: vec![task_id.clone()],
            })?,
        )?;

        let loaded = load_state(&tasks_dir, &queue_path)?;
        let queue = loaded.queue;
        let recovered = loaded.tasks.get(&task_id).expect("task loaded");

        assert!(queue.is_empty(), "stale running task must not be requeued");
        assert_eq!(recovered.status, TaskStatus::Failed);
        assert_eq!(recovered.terminal_reason.as_deref(), Some("failed"));
        assert!(
            recovered
                .error
                .as_deref()
                .is_some_and(|err| err.contains("prior process is not attached")),
            "recovered task should explain stale process ownership: {recovered:?}"
        );
        assert!(recovered.ended_at.is_some());
        assert!(recovered.duration_ms.is_some());
        assert_eq!(recovered.tool_calls[0].status, TaskToolStatus::Failed);
        assert!(recovered.tool_calls[0].ended_at.is_some());
        assert!(
            recovered
                .timeline
                .iter()
                .any(|entry| entry.kind == "recovered"
                    && entry.summary.contains("prior process is not attached")),
            "recovery timeline should explain why the task is terminal: {:?}",
            recovered.timeline
        );
        Ok(())
    }

    #[tokio::test]
    async fn default_workspace_updates_for_future_tasks() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let new_workspace =
            std::env::temp_dir().join(format!("deepseek-workspace-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        manager.set_default_workspace(new_workspace.clone()).await;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("test workspace default"))
            .await?;

        assert_eq!(manager.default_workspace().await, new_workspace);
        assert_eq!(task.workspace, new_workspace);
        Ok(())
    }

    #[tokio::test]
    async fn record_tool_metadata_updates_explicit_task() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test metadata"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        let updated = manager
            .record_tool_metadata(
                &finished.id,
                &serde_json::json!({
                    "task_updates": {
                        "gate": {
                            "id": "gate_test",
                            "gate": "test",
                            "command": "cargo test -p codewhale-tui --lib",
                            "cwd": ".",
                            "exit_code": 0,
                            "status": "passed",
                            "classification": "passed",
                            "duration_ms": 1,
                            "summary": "ok",
                            "log_path": null,
                            "recorded_at": Utc::now()
                        }
                    }
                }),
            )
            .await?;

        assert_eq!(updated.gates.len(), 1);
        assert_eq!(updated.gates[0].classification, "passed");
        Ok(())
    }

    #[tokio::test]
    async fn record_tool_metadata_updates_hunt_verdict_summary() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test verdict metadata"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        let updated = manager
            .record_tool_metadata(
                &finished.id,
                &serde_json::json!({
                    "task_updates": {
                        "hunt_verdict": "wounded"
                    }
                }),
            )
            .await?;

        assert_eq!(updated.hunt_verdict.as_deref(), Some("wounded"));
        let summaries = manager.list_tasks(Some(10)).await;
        let summary = summaries
            .iter()
            .find(|summary| summary.id == updated.id)
            .expect("updated task summary");
        assert_eq!(summary.hunt_verdict.as_deref(), Some("wounded"));
        Ok(())
    }

    #[tokio::test]
    async fn write_task_artifact_rejects_traversal_task_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("tasks-root");
        let escaped = temp.path().join("escape");
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;

        let err = manager
            .write_task_artifact("../escape", "result", "artifact body")
            .expect_err("traversal task ids must be rejected");

        assert!(err.to_string().contains("single path component"));
        assert!(!escaped.exists(), "artifact write escaped the task root");
        Ok(())
    }

    #[tokio::test]
    async fn cancel_running_task_marks_canceled() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test cancellation"))
            .await?;

        sleep(Duration::from_millis(10)).await;
        let cancellation = manager.cancel_task(&task.id).await?;
        assert_eq!(cancellation.disposition, TaskCancelDisposition::Requested);
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Canceled);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_finished_task_returns_atomic_already_finished_outcome() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("finish before cancellation"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Completed);

        let cancellation = manager.cancel_task(&task.id).await?;

        assert_eq!(
            cancellation.disposition,
            TaskCancelDisposition::AlreadyFinished
        );
        assert_eq!(cancellation.task.status, TaskStatus::Completed);
        Ok(())
    }

    // GHSA-72w5-pf8h-xfp4 — regression: omitted optional fields must not
    // silently elevate the spawned task's privileges.
    #[tokio::test]
    async fn add_task_without_optional_fields_does_not_grant_shell_or_auto_approve() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;

        let req = NewTaskRequest {
            prompt: "fix TODOs and write a README".to_string(),
            model: None,
            workspace: None,
            mode: None,
            allow_shell: None,
            trust_mode: None,
            auto_approve: None,
            owner_session_id: None,
        };
        let task = manager.add_task(req).await?;

        assert!(
            !task.allow_shell,
            "model-omitted allow_shell must default to false (no silent shell grant)"
        );
        assert!(
            !task.auto_approve,
            "model-omitted auto_approve must default to false (no silent auto-approval)"
        );
        assert!(
            !task.trust_mode,
            "model-omitted trust_mode must default to false"
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_newer_task_schema_on_recovery() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test schema gate"))
            .await?;
        let _ = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        drop(manager);

        let task_path = root.join("tasks").join(format!("{}.json", task.id));
        let mut value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&task_path)?)?;
        value["schema_version"] = serde_json::json!(999);
        fs::write(&task_path, serde_json::to_string_pretty(&value)?)?;

        match TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await {
            Ok(_) => panic!("manager should reject newer task schema"),
            Err(err) => assert!(err.to_string().contains("newer than supported")),
        }
        Ok(())
    }

    #[test]
    fn default_tasks_dir_falls_back_to_legacy_deepseek_tasks() {
        let temp_home = tempfile::tempdir().unwrap();
        let home = temp_home.path();
        let legacy_tasks = home.join(".deepseek").join("tasks");
        std::fs::create_dir_all(&legacy_tasks).unwrap();

        assert_eq!(default_tasks_dir_for_home(home), legacy_tasks);
    }

    #[test]
    fn default_tasks_dir_prefers_existing_codewhale_tasks() {
        let temp_home = tempfile::tempdir().unwrap();
        let home = temp_home.path();
        let primary_tasks = home.join(".codewhale").join("tasks");
        let legacy_tasks = home.join(".deepseek").join("tasks");
        std::fs::create_dir_all(&primary_tasks).unwrap();
        std::fs::create_dir_all(&legacy_tasks).unwrap();

        assert_eq!(default_tasks_dir_for_home(home), primary_tasks);
    }

    #[test]
    fn default_tasks_dir_falls_back_to_legacy_when_primary_is_file() {
        let temp_home = tempfile::tempdir().unwrap();
        let home = temp_home.path();
        let primary_tasks = home.join(".codewhale").join("tasks");
        let legacy_tasks = home.join(".deepseek").join("tasks");
        std::fs::create_dir_all(primary_tasks.parent().unwrap()).unwrap();
        std::fs::write(&primary_tasks, "not a directory").unwrap();
        std::fs::create_dir_all(&legacy_tasks).unwrap();

        assert_eq!(default_tasks_dir_for_home(home), legacy_tasks);
    }

    #[test]
    fn default_tasks_dir_ignores_legacy_file_for_new_installs() {
        let temp_home = tempfile::tempdir().unwrap();
        let home = temp_home.path();
        let primary_tasks = home.join(".codewhale").join("tasks");
        let legacy_tasks = home.join(".deepseek").join("tasks");
        std::fs::create_dir_all(legacy_tasks.parent().unwrap()).unwrap();
        std::fs::write(&legacy_tasks, "not a directory").unwrap();

        assert_eq!(default_tasks_dir_for_home(home), primary_tasks);
    }

    #[test]
    fn default_tasks_dir_uses_codewhale_tasks_for_new_installs() {
        let temp_home = tempfile::tempdir().unwrap();
        let home = temp_home.path();

        assert_eq!(
            default_tasks_dir_for_home(home),
            home.join(".codewhale").join("tasks")
        );
    }

    #[test]
    fn task_and_runtime_roots_honor_explicit_codewhale_home() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().unwrap();
        let ambient_home = temp_root.path().join("ambient-home");
        let explicit_home = temp_root.path().join("explicit-home");
        std::fs::create_dir_all(ambient_home.join(".deepseek").join("tasks")).unwrap();
        let _home = EnvVarGuard::set("HOME", &ambient_home);
        let _userprofile = EnvVarGuard::set("USERPROFILE", &ambient_home);
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);
        let _tasks_override = EnvVarGuard::remove("CODEWHALE_TASKS_DIR");
        let _legacy_tasks_override = EnvVarGuard::remove("DEEPSEEK_TASKS_DIR");
        let _runtime_override = EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
        let _legacy_runtime_override = EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

        let task_root = default_tasks_dir();
        let task_manager =
            TaskManagerConfig::from_runtime(&Config::default(), PathBuf::from("."), None, None);
        let runtime = RuntimeThreadManagerConfig::from_task_data_dir(task_manager.data_dir.clone());

        assert_eq!(task_root, explicit_home.join("tasks"));
        assert_eq!(task_manager.data_dir, task_root);
        assert_eq!(runtime.task_data_dir, task_root);
        assert_eq!(
            runtime.data_dir,
            explicit_home.join("tasks").join("runtime")
        );
    }

    #[test]
    fn whitespace_codewhale_home_keeps_ambient_legacy_task_and_runtime_fallbacks() {
        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().unwrap();
        let ambient_home = temp_root.path().join("ambient-home");
        let legacy_tasks = ambient_home.join(".deepseek").join("tasks");
        std::fs::create_dir_all(&legacy_tasks).unwrap();
        let _home = EnvVarGuard::set("HOME", &ambient_home);
        let _userprofile = EnvVarGuard::set("USERPROFILE", &ambient_home);
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", " \t ");
        let _tasks_override = EnvVarGuard::remove("CODEWHALE_TASKS_DIR");
        let _legacy_tasks_override = EnvVarGuard::remove("DEEPSEEK_TASKS_DIR");
        let _runtime_override = EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
        let _legacy_runtime_override = EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

        let task_root = default_tasks_dir();
        let task_manager =
            TaskManagerConfig::from_runtime(&Config::default(), PathBuf::from("."), None, None);
        let runtime = RuntimeThreadManagerConfig::from_task_data_dir(task_manager.data_dir.clone());

        assert_eq!(task_root, legacy_tasks);
        assert_eq!(task_manager.data_dir, task_root);
        assert_eq!(runtime.task_data_dir, task_root);
        assert_eq!(runtime.data_dir, task_root.join("runtime"));
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_codewhale_home_is_preserved_by_task_and_runtime_roots() {
        use std::os::unix::ffi::OsStringExt;

        let _lock = lock_test_env();
        let temp_root = tempfile::tempdir().unwrap();
        let explicit_home = temp_root.path().join(std::ffi::OsString::from_vec(
            b"codewhale-\xff-home".to_vec(),
        ));
        let _codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);
        let _tasks_override = EnvVarGuard::remove("CODEWHALE_TASKS_DIR");
        let _legacy_tasks_override = EnvVarGuard::remove("DEEPSEEK_TASKS_DIR");
        let _runtime_override = EnvVarGuard::remove("CODEWHALE_RUNTIME_DIR");
        let _legacy_runtime_override = EnvVarGuard::remove("DEEPSEEK_RUNTIME_DIR");

        let task_root = default_tasks_dir();
        let task_manager =
            TaskManagerConfig::from_runtime(&Config::default(), PathBuf::from("."), None, None);
        let runtime = RuntimeThreadManagerConfig::from_task_data_dir(task_manager.data_dir.clone());

        assert_eq!(task_root, explicit_home.join("tasks"));
        assert_eq!(task_manager.data_dir, task_root);
        assert_eq!(runtime.task_data_dir, task_root);
        assert_eq!(
            runtime.data_dir,
            explicit_home.join("tasks").join("runtime")
        );
    }

    struct DeafHangExecutor;

    #[async_trait]
    impl TaskExecutor for DeafHangExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            _events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            std::future::pending().await
        }
    }

    struct PartialThenHangExecutor;

    #[async_trait]
    impl TaskExecutor for PartialThenHangExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            let _ = events
                .send(TaskExecutionEvent::MessageDelta {
                    content: "partial ".to_string(),
                })
                .await;
            let _ = events
                .send(TaskExecutionEvent::MessageDelta {
                    content: "result".to_string(),
                })
                .await;
            std::future::pending().await
        }
    }

    struct PollCountingHangExecutor {
        polls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl TaskExecutor for PollCountingHangExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            _events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            std::future::poll_fn(|_| {
                self.polls.fetch_add(1, Ordering::Relaxed);
                std::task::Poll::Pending
            })
            .await
        }
    }

    struct HeartbeatExecutor;

    #[async_trait]
    impl TaskExecutor for HeartbeatExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            loop {
                let _ = events
                    .send(TaskExecutionEvent::Status {
                        message: "heartbeat".to_string(),
                    })
                    .await;
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    struct ProgressHeartbeatExecutor;

    #[async_trait]
    impl TaskExecutor for ProgressHeartbeatExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            loop {
                let _ = events
                    .send(TaskExecutionEvent::MessageDelta {
                        content: "working".to_string(),
                    })
                    .await;
                sleep(Duration::from_millis(10)).await;
            }
        }
    }

    struct CooperativeIdleCancelExecutor;

    #[async_trait]
    impl TaskExecutor for CooperativeIdleCancelExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            _events: mpsc::Sender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            cancel.cancelled().await;
            TaskExecutionResult::from_reason(TaskTerminalReason::Canceled, None)
        }
    }

    struct CooperativeProgressCancelExecutor;

    #[async_trait]
    impl TaskExecutor for CooperativeProgressCancelExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        return TaskExecutionResult::from_reason(
                            TaskTerminalReason::Canceled,
                            None,
                        );
                    }
                    _ = sleep(Duration::from_millis(10)) => {
                        let _ = events
                            .send(TaskExecutionEvent::MessageDelta {
                                content: "working".to_string(),
                            })
                            .await;
                    }
                }
            }
        }
    }

    struct PromptRouterExecutor;

    #[async_trait]
    impl TaskExecutor for PromptRouterExecutor {
        async fn execute(
            &self,
            task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            if task.prompt.starts_with("hang ") {
                std::future::pending().await
            } else {
                MockExecutor.execute(task, events, cancel).await
            }
        }
    }

    struct FloodExecutor;

    #[async_trait]
    impl TaskExecutor for FloodExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            events: mpsc::Sender<TaskExecutionEvent>,
            _cancel: CancellationToken,
        ) -> TaskExecutionResult {
            for i in 0..400 {
                // Mirror the runtime path: each raw event is followed by its
                // derived message delta. Alternating the two non-urgent stream
                // kinds prevents timeline coalescing without turning this
                // storage-bound test into hundreds of synchronous fsyncs.
                let _ = events
                    .send(TaskExecutionEvent::RuntimeEvent {
                        seq: i,
                        event: "item.delta".to_string(),
                        summary: format!("tick {i}"),
                    })
                    .await;
                let _ = events
                    .send(TaskExecutionEvent::MessageDelta {
                        content: format!("chunk {i}"),
                    })
                    .await;
            }
            TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("flooded".to_string()),
                error: None,
                terminal_reason: TaskTerminalReason::Completed,
            }
        }
    }

    struct CompleteAfterCancelExecutor;

    #[async_trait]
    impl TaskExecutor for CompleteAfterCancelExecutor {
        async fn execute(
            &self,
            _task: ExecutionTask,
            _events: mpsc::Sender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            cancel.cancelled().await;
            TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("late complete".to_string()),
                error: None,
                terminal_reason: TaskTerminalReason::Completed,
            }
        }
    }

    fn sample_task_record() -> TaskRecord {
        TaskRecord {
            schema_version: CURRENT_TASK_SCHEMA_VERSION,
            id: "task_0123456789abcdef".to_string(),
            prompt: "bound timeline".to_string(),
            model: "deepseek-v4-flash".to_string(),
            workspace: PathBuf::from("."),
            mode: "agent".to_string(),
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            status: TaskStatus::Running,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            ended_at: None,
            duration_ms: None,
            hunt_verdict: None,
            result_summary: None,
            result_detail_path: None,
            error: None,
            terminal_reason: None,
            thread_id: None,
            turn_id: None,
            owner_session_id: None,
            runtime_event_count: 0,
            lifecycle_seq: 2,
            checklist: TaskChecklistState::default(),
            gates: Vec::new(),
            attempts: Vec::new(),
            artifacts: Vec::new(),
            github_events: Vec::new(),
            tool_calls: Vec::new(),
            timeline: Vec::new(),
        }
    }

    #[test]
    fn execution_guard_idle_does_not_reset_without_progress() {
        let start = Instant::now();
        let limits = TaskExecutionLimits::short_for_tests();
        let guard = ExecutionGuard::new(limits, start);
        match guard.evaluate(start + limits.idle_progress, false, false) {
            GuardAction::Interrupt { reason } => {
                assert_eq!(reason, TaskTerminalReason::IdleTimeout);
            }
            other => panic!("expected idle interrupt, got {other:?}"),
        }
    }

    #[test]
    fn execution_guard_progress_refreshes_idle_until_wall_timeout() {
        let start = Instant::now();
        let limits = TaskExecutionLimits::short_for_tests();
        let mut guard = ExecutionGuard::new(limits, start);
        let progressed = start + (limits.idle_progress / 2);
        guard.note_progress(progressed);
        match guard.evaluate(progressed + (limits.idle_progress / 2), false, false) {
            GuardAction::Run { .. } => {}
            other => panic!("progress should keep idle from firing, got {other:?}"),
        }
        match guard.evaluate(start + limits.wall_time, false, false) {
            GuardAction::Interrupt { reason } => {
                assert_eq!(reason, TaskTerminalReason::WallTimeout);
            }
            other => panic!("expected wall interrupt, got {other:?}"),
        }
    }

    #[test]
    fn execution_guard_cancel_grace_terminalizes_stuck_work() {
        let start = Instant::now();
        let limits = TaskExecutionLimits::short_for_tests();
        let mut guard = ExecutionGuard::new(limits, start);
        match guard.evaluate(start, true, false) {
            GuardAction::Interrupt { reason } => {
                assert_eq!(reason, TaskTerminalReason::Canceled);
                guard.note_interrupt(start, reason);
            }
            other => panic!("expected cancel interrupt, got {other:?}"),
        }
        match guard.evaluate(start + limits.cancel_grace, true, false) {
            GuardAction::Terminalize { reason } => {
                assert_eq!(reason, TaskTerminalReason::CancelTimeout);
            }
            other => panic!("expected cancel timeout, got {other:?}"),
        }
    }

    #[test]
    fn consecutive_message_deltas_coalesce_on_the_timeline() {
        let mut task = sample_task_record();
        for i in 0..50 {
            push_timeline_entry(
                &mut task,
                TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "message".to_string(),
                    summary: format!("chunk {i}"),
                    detail_path: None,
                },
            );
        }
        assert_eq!(
            task.timeline
                .iter()
                .filter(|entry| entry.kind == "message")
                .count(),
            1
        );
        assert_eq!(
            task.timeline.last().map(|e| e.summary.as_str()),
            Some("chunk 49")
        );
    }

    #[test]
    fn timeline_trim_bounds_growth_and_keeps_a_head() {
        let mut task = sample_task_record();
        for i in 0..400 {
            push_timeline_entry(
                &mut task,
                TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "status".to_string(),
                    summary: format!("tick {i}"),
                    detail_path: None,
                },
            );
        }
        assert!(task.timeline.len() <= TIMELINE_ENTRY_LIMIT);
        assert_eq!(task.timeline[0].summary, "tick 0");
        assert!(
            task.timeline.iter().any(|entry| entry.kind == "omitted"),
            "bounded timeline should record omitted history: {:?}",
            task.timeline
                .iter()
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn never_terminalizing_execution_fails_with_idle_timeout() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(short_test_config(root), Arc::new(DeafHangExecutor))
                .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("never finish"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("idle_timeout"));
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|err| err.contains("idle")),
            "idle timeout must be visible on the receipt: {finished:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn forced_timeout_keeps_all_partial_message_output() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            short_test_config(root),
            Arc::new(PartialThenHangExecutor),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("retain partial result"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;

        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("idle_timeout"));
        assert_eq!(finished.result_summary.as_deref(), Some("partial result"));
        Ok(())
    }

    #[tokio::test]
    async fn cooperative_cancel_after_idle_timeout_keeps_timeout_reason() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            short_test_config(root),
            Arc::new(CooperativeIdleCancelExecutor),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("cooperative idle timeout"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("idle_timeout"));
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|error| error.contains("idle")),
            "cooperative cancellation must retain the timeout receipt: {finished:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn heartbeat_status_does_not_refresh_idle_timeout() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(short_test_config(root), Arc::new(HeartbeatExecutor))
                .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("heartbeat only"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("idle_timeout"));
        Ok(())
    }

    #[tokio::test]
    async fn active_progress_keeps_idle_alive_until_wall_timeout() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            short_test_config(root),
            Arc::new(ProgressHeartbeatExecutor),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("genuine progress"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("wall_timeout"));
        Ok(())
    }

    #[tokio::test]
    async fn cooperative_cancel_after_wall_timeout_keeps_timeout_reason() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            wall_timeout_test_config(root),
            Arc::new(CooperativeProgressCancelExecutor),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("cooperative wall timeout"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Failed);
        assert_eq!(finished.terminal_reason.as_deref(), Some("wall_timeout"));
        assert!(
            finished
                .error
                .as_deref()
                .is_some_and(|error| error.contains("wall-time")),
            "cooperative cancellation must retain the timeout receipt: {finished:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_terminalizes_a_stuck_running_task() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(short_test_config(root), Arc::new(DeafHangExecutor))
                .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("stuck during shutdown"))
            .await?;
        sleep(Duration::from_millis(5)).await;
        manager.shutdown();
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Canceled);
        assert_eq!(finished.terminal_reason.as_deref(), Some("shutdown"));
        Ok(())
    }

    #[tokio::test]
    async fn shutdown_cancel_signal_does_not_spin_during_grace() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let polls = Arc::new(AtomicUsize::new(0));
        let manager = TaskManager::start_with_executor(
            short_test_config(root),
            Arc::new(PollCountingHangExecutor {
                polls: Arc::clone(&polls),
            }),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("stuck during shutdown"))
            .await?;

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while manager.get_task(&task.id).await?.status != TaskStatus::Running {
            if std::time::Instant::now() >= deadline {
                bail!("task never started running");
            }
            sleep(Duration::from_millis(5)).await;
        }

        manager.shutdown();
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.terminal_reason.as_deref(), Some("shutdown"));
        assert!(
            polls.load(Ordering::Relaxed) <= 10,
            "already-canceled shutdown signal repeatedly repolled the executor during grace: {} polls",
            polls.load(Ordering::Relaxed)
        );
        Ok(())
    }

    #[tokio::test]
    async fn forced_idle_timeout_releases_the_worker_for_later_tasks() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            short_test_config(root),
            Arc::new(PromptRouterExecutor),
        )
        .await?;
        let stuck = manager
            .add_task(NewTaskRequest::from_prompt("hang until idle timeout"))
            .await?;
        let finished =
            wait_for_terminal_state(&manager, &stuck.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.terminal_reason.as_deref(), Some("idle_timeout"));

        let next = manager
            .add_task(NewTaskRequest::from_prompt("run after hang"))
            .await?;
        let completed =
            wait_for_terminal_state(&manager, &next.id, Duration::from_secs(10)).await?;
        assert_eq!(completed.status, TaskStatus::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn cancel_then_completed_result_is_recorded_as_canceled() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager = TaskManager::start_with_executor(
            test_config(root),
            Arc::new(CompleteAfterCancelExecutor),
        )
        .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("race complete after cancel"))
            .await?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let current = manager.get_task(&task.id).await?;
            if current.status == TaskStatus::Running {
                break;
            }
            if std::time::Instant::now() >= deadline {
                bail!("task never started running");
            }
            sleep(Duration::from_millis(5)).await;
        }
        sleep(Duration::from_millis(5)).await;
        let cancellation = manager.cancel_task(&task.id).await?;
        assert_eq!(cancellation.disposition, TaskCancelDisposition::Requested);
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Canceled);
        assert_eq!(finished.terminal_reason.as_deref(), Some("canceled"));
        Ok(())
    }

    #[tokio::test]
    async fn long_stream_timeline_is_bounded() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(FloodExecutor))
                .await?;
        let task = manager
            .add_task(NewTaskRequest::from_prompt("flood the timeline"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(finished.runtime_event_count, 400);
        assert!(
            finished.timeline.len() <= TIMELINE_ENTRY_LIMIT,
            "timeline grew to {}",
            finished.timeline.len()
        );
        assert!(
            finished
                .timeline
                .iter()
                .any(|entry| entry.kind == "omitted"),
            "long streams should drop older timeline entries: {:?}",
            finished
                .timeline
                .iter()
                .map(|e| e.kind.as_str())
                .collect::<Vec<_>>()
        );

        let persisted_path = root.join("tasks").join(format!("{}.json", task.id));
        let persisted: TaskRecord = serde_json::from_slice(&fs::read(&persisted_path)?)?;
        assert_eq!(persisted.status, TaskStatus::Completed);
        assert_eq!(persisted.runtime_event_count, 400);
        assert!(persisted.timeline.len() <= TIMELINE_ENTRY_LIMIT);
        assert!(
            persisted
                .timeline
                .iter()
                .any(|entry| entry.kind == "omitted")
        );
        Ok(())
    }

    async fn test_runtime_manager() -> Result<RuntimeThreadManager> {
        let root = tempfile::tempdir()?.keep();
        RuntimeThreadManager::open(
            Config::default(),
            PathBuf::from("."),
            RuntimeThreadManagerConfig::from_task_data_dir(root),
        )
    }

    async fn drain_task_events(mut rx: mpsc::Receiver<TaskExecutionEvent>) {
        while rx.recv().await.is_some() {}
    }

    #[tokio::test]
    async fn engine_turn_without_terminal_event_idle_times_out() -> Result<()> {
        let runtime = test_runtime_manager().await?;
        let thread = runtime
            .create_thread(CreateThreadRequest::default())
            .await?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(drain_task_events(rx));
        let result = drive_engine_turn(
            &runtime,
            &thread.id,
            "turn_missing",
            tx,
            CancellationToken::new(),
            TaskExecutionLimits::short_for_tests(),
        )
        .await;
        assert_eq!(result.status, TaskStatus::Failed);
        assert_eq!(result.terminal_reason, TaskTerminalReason::IdleTimeout);
        Ok(())
    }

    #[tokio::test]
    async fn engine_turn_keeps_idle_timeout_when_runtime_interrupts_during_grace() -> Result<()> {
        let runtime = Arc::new(test_runtime_manager().await?);
        let thread = runtime
            .create_thread(CreateThreadRequest::default())
            .await?;
        let (tx, mut rx) = mpsc::channel(64);
        let runtime_for_drive = Arc::clone(&runtime);
        let thread_id = thread.id.clone();
        let drive = tokio::spawn(async move {
            drive_engine_turn(
                runtime_for_drive.as_ref(),
                &thread_id,
                "turn_timeout",
                tx,
                CancellationToken::new(),
                TaskExecutionLimits {
                    wall_time: Duration::from_secs(2),
                    idle_progress: Duration::from_millis(80),
                    cancel_grace: Duration::from_millis(500),
                    persist_debounce: Duration::from_millis(10),
                },
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match rx.recv().await {
                    Some(TaskExecutionEvent::Status { message })
                        if message.contains("idle deadline") =>
                    {
                        break;
                    }
                    Some(_) => {}
                    None => panic!("engine task event stream closed before timeout interrupt"),
                }
            }
        })
        .await
        .context("engine did not request the idle-timeout interrupt")?;

        runtime
            .emit_event_for_test(
                &thread.id,
                Some("turn_timeout"),
                "turn.completed",
                json!({ "turn": { "status": "interrupted" } }),
            )
            .await?;
        let result = drive.await?;
        assert_eq!(result.status, TaskStatus::Failed);
        assert_eq!(result.terminal_reason, TaskTerminalReason::IdleTimeout);
        Ok(())
    }

    #[tokio::test]
    async fn engine_turn_uses_cursor_catchup_for_completed_event() -> Result<()> {
        let runtime = test_runtime_manager().await?;
        let thread = runtime
            .create_thread(CreateThreadRequest::default())
            .await?;
        runtime
            .emit_event_for_test(
                &thread.id,
                Some("turn_done"),
                "turn.completed",
                json!({ "turn": { "status": "completed" } }),
            )
            .await?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(drain_task_events(rx));
        let result = drive_engine_turn(
            &runtime,
            &thread.id,
            "turn_done",
            tx,
            CancellationToken::new(),
            TaskExecutionLimits::short_for_tests(),
        )
        .await;
        assert_eq!(result.status, TaskStatus::Completed);
        assert_eq!(result.terminal_reason, TaskTerminalReason::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn engine_turn_prefers_runtime_terminal_over_cancel_grace() -> Result<()> {
        let runtime = test_runtime_manager().await?;
        let thread = runtime
            .create_thread(CreateThreadRequest::default())
            .await?;
        let cancel = CancellationToken::new();
        cancel.cancel();
        runtime
            .emit_event_for_test(
                &thread.id,
                Some("turn_done"),
                "turn.completed",
                json!({ "turn": { "status": "interrupted" } }),
            )
            .await?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(drain_task_events(rx));
        let result = drive_engine_turn(
            &runtime,
            &thread.id,
            "turn_done",
            tx,
            cancel,
            TaskExecutionLimits::short_for_tests(),
        )
        .await;
        assert_eq!(result.status, TaskStatus::Canceled);
        assert_eq!(result.terminal_reason, TaskTerminalReason::Canceled);
        Ok(())
    }
}
