//! Persistent background task manager for DeepSeek agent work.
//!
//! Tasks are durable across restarts and execute with a bounded worker pool.
//! Execution stays DeepSeek-only and now links every task to runtime
//! thread/turn records for unified timelines.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{Config, DEFAULT_TEXT_MODEL, MAX_SUBAGENTS};
use crate::runtime_threads::{
    CreateThreadRequest, RuntimeThreadManager, RuntimeThreadManagerConfig, RuntimeTurnStatus,
    SharedRuntimeThreadManager, StartTurnRequest,
};
use crate::utils::spawn_supervised;

const DEFAULT_WORKERS: usize = 2;
const MAX_WORKERS: usize = 8;
const TIMELINE_SUMMARY_LIMIT: usize = 240;
const ARTIFACT_THRESHOLD: usize = 1200;
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

impl TaskStatus {
    #[cfg(test)]
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_detail_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default)]
    pub runtime_event_count: usize,
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
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
}

impl From<&TaskRecord> for TaskSummary {
    fn from(value: &TaskRecord) -> Self {
        Self {
            id: value.id.clone(),
            status: value.status,
            prompt_summary: summarize_text(&value.prompt, TIMELINE_SUMMARY_LIMIT),
            model: value.model.clone(),
            mode: value.mode.clone(),
            created_at: value.created_at,
            started_at: value.started_at,
            ended_at: value.ended_at,
            duration_ms: value.duration_ms,
            error: value.error.clone(),
            thread_id: value.thread_id.clone(),
            turn_id: value.turn_id.clone(),
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
    #[allow(dead_code)]
    pub max_subagents: usize,
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
            default_model: default_model.unwrap_or_else(|| {
                config
                    .default_text_model
                    .clone()
                    .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string())
            }),
            default_mode: "agent".to_string(),
            allow_shell: config.allow_shell(),
            trust_mode: false,
            max_subagents: config.max_subagents().clamp(1, MAX_SUBAGENTS),
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
}

/// Abstraction for task execution.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::UnboundedSender<TaskExecutionEvent>,
        cancel: CancellationToken,
    ) -> TaskExecutionResult;
}

/// Engine-backed executor (DeepSeek-only).
pub struct EngineTaskExecutor {
    runtime_threads: SharedRuntimeThreadManager,
}

impl EngineTaskExecutor {
    #[must_use]
    pub fn new(runtime_threads: SharedRuntimeThreadManager) -> Self {
        Self { runtime_threads }
    }
}

#[async_trait]
impl TaskExecutor for EngineTaskExecutor {
    async fn execute(
        &self,
        task: ExecutionTask,
        events: mpsc::UnboundedSender<TaskExecutionEvent>,
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
            })
            .await
        {
            Ok(thread) => thread,
            Err(err) => {
                return TaskExecutionResult {
                    status: TaskStatus::Failed,
                    result_text: None,
                    error: Some(format!("Failed to create runtime thread: {err}")),
                };
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
                },
            )
            .await
        {
            Ok(turn) => turn,
            Err(err) => {
                return TaskExecutionResult {
                    status: TaskStatus::Failed,
                    result_text: None,
                    error: Some(format!("Failed to start task: {err}")),
                };
            }
        };

        let _ = events.send(TaskExecutionEvent::ThreadLinked {
            thread_id: thread.id.clone(),
            turn_id: turn.id.clone(),
        });
        let _ = events.send(TaskExecutionEvent::Status {
            message: format!("Task {} started", task.id),
        });

        let mut final_text = String::new();
        let mut seen_seq = 0u64;
        let mut cancel_requested = false;
        let mut terminal_status: Option<RuntimeTurnStatus> = None;
        let mut terminal_error: Option<String> = None;

        loop {
            if cancel.is_cancelled() && !cancel_requested {
                cancel_requested = true;
                let _ = self
                    .runtime_threads
                    .interrupt_turn(&thread.id, &turn.id)
                    .await;
                let _ = events.send(TaskExecutionEvent::Status {
                    message: "Cancellation requested".to_string(),
                });
            }

            let batch = match self
                .runtime_threads
                .events_since(&thread.id, Some(seen_seq))
            {
                Ok(batch) => batch,
                Err(err) => {
                    return TaskExecutionResult {
                        status: TaskStatus::Failed,
                        result_text: if final_text.trim().is_empty() {
                            None
                        } else {
                            Some(final_text)
                        },
                        error: Some(format!("Failed to read runtime events: {err}")),
                    };
                }
            };

            for event in batch {
                seen_seq = seen_seq.max(event.seq);
                let _ = events.send(TaskExecutionEvent::RuntimeEvent {
                    seq: event.seq,
                    event: event.event.clone(),
                    summary: summarize_text(&event.payload.to_string(), TIMELINE_SUMMARY_LIMIT),
                });

                match event.event.as_str() {
                    "item.delta" => {
                        let kind = event
                            .payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if kind == "agent_message" {
                            if let Some(content) =
                                event.payload.get("delta").and_then(Value::as_str)
                            {
                                final_text.push_str(content);
                                let _ = events.send(TaskExecutionEvent::MessageDelta {
                                    content: content.to_string(),
                                });
                            }
                        } else if kind == "tool_call" {
                            let output = event
                                .payload
                                .get("delta")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string();
                            let _ = events.send(TaskExecutionEvent::ToolProgress {
                                id: event.item_id.clone().unwrap_or_default(),
                                output,
                            });
                        }
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
                            let _ =
                                events.send(TaskExecutionEvent::ToolStarted { id, name, input });
                        }
                    }
                    "item.completed" | "item.failed" => {
                        if let Some(item) = event.payload.get("item") {
                            let kind = item.get("kind").and_then(Value::as_str).unwrap_or_default();
                            if kind == "tool_call"
                                || kind == "file_change"
                                || kind == "command_execution"
                            {
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
                                let _ = events.send(TaskExecutionEvent::ToolCompleted {
                                    id,
                                    name,
                                    success: event.event == "item.completed",
                                    output,
                                    metadata,
                                });
                            } else if kind == "status" {
                                let message = item
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .or_else(|| item.get("summary").and_then(Value::as_str))
                                    .unwrap_or_default()
                                    .to_string();
                                let _ = events.send(TaskExecutionEvent::Status { message });
                            } else if kind == "error" {
                                let message = item
                                    .get("detail")
                                    .and_then(Value::as_str)
                                    .or_else(|| item.get("summary").and_then(Value::as_str))
                                    .unwrap_or_default()
                                    .to_string();
                                let _ = events.send(TaskExecutionEvent::Error { message });
                            }
                        }
                    }
                    "turn.completed" => {
                        if let Some(turn_payload) = event.payload.get("turn") {
                            let status = turn_payload
                                .get("status")
                                .and_then(Value::as_str)
                                .unwrap_or("failed");
                            terminal_status = Some(match status {
                                "completed" => RuntimeTurnStatus::Completed,
                                "interrupted" => RuntimeTurnStatus::Interrupted,
                                "canceled" => RuntimeTurnStatus::Canceled,
                                _ => RuntimeTurnStatus::Failed,
                            });
                            terminal_error = turn_payload
                                .get("error")
                                .and_then(Value::as_str)
                                .map(ToString::to_string);
                        } else {
                            terminal_status = Some(RuntimeTurnStatus::Completed);
                        }
                    }
                    _ => {}
                }
            }

            if terminal_status.is_some() {
                break;
            }

            sleep(Duration::from_millis(40)).await;
        }

        match terminal_status.unwrap_or(RuntimeTurnStatus::Failed) {
            RuntimeTurnStatus::Completed => TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: if final_text.trim().is_empty() {
                    None
                } else {
                    Some(final_text)
                },
                error: None,
            },
            RuntimeTurnStatus::Interrupted | RuntimeTurnStatus::Canceled => TaskExecutionResult {
                status: TaskStatus::Canceled,
                result_text: if final_text.trim().is_empty() {
                    None
                } else {
                    Some(final_text)
                },
                error: None,
            },
            RuntimeTurnStatus::Queued
            | RuntimeTurnStatus::InProgress
            | RuntimeTurnStatus::Failed => TaskExecutionResult {
                status: TaskStatus::Failed,
                result_text: if final_text.trim().is_empty() {
                    None
                } else {
                    Some(final_text)
                },
                error: terminal_error.or_else(|| Some("Task ended unexpectedly".to_string())),
            },
        }
    }
}

/// Thread-safe task manager.
pub type SharedTaskManager = Arc<TaskManager>;

pub struct TaskManager {
    cfg: TaskManagerConfig,
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
    pub async fn start(cfg: TaskManagerConfig, api_config: Config) -> Result<SharedTaskManager> {
        let runtime_threads = Arc::new(RuntimeThreadManager::open(
            api_config.clone(),
            cfg.default_workspace.clone(),
            RuntimeThreadManagerConfig::from_task_data_dir(cfg.data_dir.clone()),
        )?);
        Self::start_with_runtime_manager(cfg, api_config, runtime_threads).await
    }

    /// Start the manager with an injected runtime thread manager.
    pub async fn start_with_runtime_manager(
        cfg: TaskManagerConfig,
        _api_config: Config,
        runtime_threads: SharedRuntimeThreadManager,
    ) -> Result<SharedTaskManager> {
        let executor: Arc<dyn TaskExecutor> =
            Arc::new(EngineTaskExecutor::new(runtime_threads.clone()));
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

        let (tasks, queue) = load_state(&tasks_dir, &queue_path)?;

        let cancel_token = CancellationToken::new();
        let manager = Arc::new(Self {
            cfg,
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
            let state = manager.state.lock().await;
            manager.persist_all_locked(&state)?;
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

    #[allow(dead_code)] // Public API for external callers (runtime API)
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }

    #[allow(dead_code)] // Public API for external callers
    pub fn is_shutdown(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Enqueue a new task.
    pub async fn add_task(&self, req: NewTaskRequest) -> Result<TaskRecord> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("Task prompt cannot be empty");
        }

        let task = TaskRecord {
            schema_version: CURRENT_TASK_SCHEMA_VERSION,
            id: format!("task_{}", &Uuid::new_v4().to_string()[..8]),
            prompt,
            model: req.model.unwrap_or_else(|| self.cfg.default_model.clone()),
            workspace: req
                .workspace
                .unwrap_or_else(|| self.cfg.default_workspace.clone()),
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
            result_summary: None,
            result_detail_path: None,
            error: None,
            thread_id: None,
            turn_id: None,
            runtime_event_count: 0,
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
            state.queue.push_back(task.id.clone());
            state.tasks.insert(task.id.clone(), task.clone());
            self.persist_all_locked(&state)?;
        }
        self.notify.notify_one();
        Ok(task)
    }

    /// List tasks, newest first.
    pub async fn list_tasks(&self, limit: Option<usize>) -> Vec<TaskSummary> {
        let state = self.state.lock().await;
        let mut items = state
            .tasks
            .values()
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
        let state = self.state.lock().await;
        let id = resolve_task_id(&state.tasks, id_or_prefix)?;
        state
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("Task not found: {id_or_prefix}"))
    }

    /// Cancel a queued or running task by id/prefix.
    pub async fn cancel_task(&self, id_or_prefix: &str) -> Result<TaskRecord> {
        let mut state = self.state.lock().await;
        let id = resolve_task_id(&state.tasks, id_or_prefix)?;
        let now = Utc::now();

        let mut cancel_running = false;
        {
            let task = state
                .tasks
                .get_mut(&id)
                .ok_or_else(|| anyhow!("Task not found: {id}"))?;
            match task.status {
                TaskStatus::Queued => {
                    task.status = TaskStatus::Canceled;
                    task.ended_at = Some(now);
                    task.duration_ms = Some(0);
                    task.timeline.push(TaskTimelineEntry {
                        timestamp: now,
                        kind: "canceled".to_string(),
                        summary: "Task canceled before execution".to_string(),
                        detail_path: None,
                    });
                    state.queue.retain(|queued_id| queued_id != &id);
                }
                TaskStatus::Running => {
                    cancel_running = true;
                    task.timeline.push(TaskTimelineEntry {
                        timestamp: now,
                        kind: "cancel_requested".to_string(),
                        summary: "Cancellation requested".to_string(),
                        detail_path: None,
                    });
                }
                _ => {}
            }
        }

        if cancel_running && let Some(token) = state.running_cancel.get(&id) {
            token.cancel();
        }

        self.persist_all_locked(&state)?;
        state
            .tasks
            .get(&id)
            .cloned()
            .ok_or_else(|| anyhow!("Task not found: {id}"))
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
                                task.started_at = Some(now);
                                task.ended_at = None;
                                task.duration_ms = None;
                                task.error = None;
                                task.timeline.push(TaskTimelineEntry {
                                    timestamp: now,
                                    kind: "running".to_string(),
                                    summary: "Task started".to_string(),
                                    detail_path: None,
                                });

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
        let (event_tx, mut event_rx) = mpsc::unbounded_channel();
        let exec_fut = self
            .executor
            .execute(request.clone(), event_tx, cancel.clone());
        tokio::pin!(exec_fut);

        let result = loop {
            tokio::select! {
                maybe_event = event_rx.recv() => {
                    if let Some(event) = maybe_event
                        && let Err(err) = self.apply_execution_event(&task_id, event).await
                    {
                        tracing::error!("Failed to apply task event for {task_id}: {err}");
                    }
                }
                exec_result = &mut exec_fut => {
                    break exec_result;
                }
            }
        };

        while let Ok(event) = event_rx.try_recv() {
            if let Err(err) = self.apply_execution_event(&task_id, event).await {
                tracing::error!("Failed to apply trailing task event for {task_id}: {err}");
            }
        }

        if let Err(err) = self
            .finish_task(&task_id, result, cancel, &request.mode_label)
            .await
        {
            tracing::error!("Failed to finalize task {task_id}: {err}");
        }
    }

    async fn apply_execution_event(&self, task_id: &str, event: TaskExecutionEvent) -> Result<()> {
        let mut state = self.state.lock().await;
        let Some(task) = state.tasks.get_mut(task_id) else {
            return Ok(());
        };

        match event {
            TaskExecutionEvent::ThreadLinked { thread_id, turn_id } => {
                task.thread_id = Some(thread_id.clone());
                task.turn_id = Some(turn_id.clone());
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "runtime_link".to_string(),
                    summary: format!("Linked runtime thread {thread_id} turn {turn_id}"),
                    detail_path: None,
                });
            }
            TaskExecutionEvent::Status { message } => {
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "status".to_string(),
                    summary: summarize_text(&message, TIMELINE_SUMMARY_LIMIT),
                    detail_path: None,
                });
            }
            TaskExecutionEvent::MessageDelta { content } => {
                if !content.trim().is_empty() {
                    task.timeline.push(TaskTimelineEntry {
                        timestamp: Utc::now(),
                        kind: "message".to_string(),
                        summary: summarize_text(&content, TIMELINE_SUMMARY_LIMIT),
                        detail_path: None,
                    });
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
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "tool_started".to_string(),
                    summary,
                    detail_path: None,
                });
            }
            TaskExecutionEvent::ToolProgress { id, output } => {
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "tool_progress".to_string(),
                    summary: format!(
                        "{id}: {}",
                        summarize_text(&output, TIMELINE_SUMMARY_LIMIT.saturating_sub(8))
                    ),
                    detail_path: None,
                });
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
                task.timeline.push(TaskTimelineEntry {
                    timestamp: now,
                    kind: "tool_completed".to_string(),
                    summary: format!("{name} {status}: {output_summary}"),
                    detail_path: detail_path.clone(),
                });
                if let Some(patch_ref) = patch_ref {
                    task.timeline.push(TaskTimelineEntry {
                        timestamp: now,
                        kind: "patch_ref".to_string(),
                        summary: format!("Patch artifact: {}", patch_ref.display()),
                        detail_path: Some(patch_ref),
                    });
                }

                self.apply_task_update_metadata(task, metadata.as_ref())?;
            }
            TaskExecutionEvent::Error { message } => {
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "error".to_string(),
                    summary: summarize_text(&message, TIMELINE_SUMMARY_LIMIT),
                    detail_path: None,
                });
            }
            TaskExecutionEvent::RuntimeEvent {
                seq,
                event,
                summary,
            } => {
                task.runtime_event_count = task.runtime_event_count.saturating_add(1);
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "runtime_event".to_string(),
                    summary: format!("#{seq} {event}: {summary}"),
                    detail_path: None,
                });
            }
        }

        self.persist_task_locked(task)?;
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
        }

        task.status = result.status;
        task.mode = mode_label.to_string();
        task.ended_at = Some(now);
        task.duration_ms = task.started_at.map(|start| duration_ms(start, now));
        task.error = result.error.clone();
        task.timeline.push(TaskTimelineEntry {
            timestamp: now,
            kind: "finished".to_string(),
            summary: match result.status {
                TaskStatus::Completed => "Task completed".to_string(),
                TaskStatus::Failed => format!(
                    "Task failed: {}",
                    result
                        .error
                        .as_deref()
                        .map(|e| summarize_text(e, TIMELINE_SUMMARY_LIMIT))
                        .unwrap_or_else(|| "unknown error".to_string())
                ),
                TaskStatus::Canceled => "Task canceled".to_string(),
                TaskStatus::Queued | TaskStatus::Running => {
                    format!("Task ended in unexpected state: {}", mode_label)
                }
            },
            detail_path: None,
        });

        if let Some(text) = result.result_text {
            let detail_path = self.artifact_if_large(task_id, "result", &text)?;
            task.result_summary = Some(summarize_text(&text, TIMELINE_SUMMARY_LIMIT));
            task.result_detail_path = detail_path.clone();
            if let Some(detail_path) = detail_path {
                task.timeline.push(TaskTimelineEntry {
                    timestamp: now,
                    kind: "result_ref".to_string(),
                    summary: format!("Result artifact: {}", detail_path.display()),
                    detail_path: Some(detail_path),
                });
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
            task.timeline.push(TaskTimelineEntry {
                timestamp: now,
                kind: "checklist".to_string(),
                summary: format!(
                    "Checklist updated: {} item(s), {}% complete",
                    task.checklist.items.len(),
                    task.checklist.completion_pct
                ),
                detail_path: None,
            });
        }

        if let Some(value) = updates.get("gate") {
            let gate: TaskGateRecord = serde_json::from_value(value.clone())
                .context("Failed to parse gate task update")?;
            let summary = format!("Gate {} {}: {}", gate.gate, gate.status, gate.summary);
            task.gates.retain(|existing| existing.id != gate.id);
            task.gates.push(gate.clone());
            task.timeline.push(TaskTimelineEntry {
                timestamp: now,
                kind: "gate".to_string(),
                summary: summarize_text(&summary, TIMELINE_SUMMARY_LIMIT),
                detail_path: gate.log_path,
            });
        }

        if let Some(value) = updates.get("attempt") {
            let attempt: TaskAttemptRecord = serde_json::from_value(value.clone())
                .context("Failed to parse attempt task update")?;
            task.attempts.retain(|existing| existing.id != attempt.id);
            task.attempts.push(attempt.clone());
            task.timeline.push(TaskTimelineEntry {
                timestamp: now,
                kind: "pr_attempt".to_string(),
                summary: format!(
                    "Attempt {}/{} recorded for {}",
                    attempt.attempt_index, attempt.attempt_count, attempt.attempt_group_id
                ),
                detail_path: attempt.patch_path,
            });
        }

        if let Some(value) = updates.get("artifacts")
            && let Some(items) = value.as_array()
        {
            for item in items {
                let artifact: TaskArtifactRef = serde_json::from_value(item.clone())
                    .context("Failed to parse artifact task update")?;
                task.timeline.push(TaskTimelineEntry {
                    timestamp: now,
                    kind: "artifact".to_string(),
                    summary: format!("{}: {}", artifact.label, artifact.summary),
                    detail_path: Some(artifact.path.clone()),
                });
                task.artifacts.push(artifact);
            }
        }

        if let Some(value) = updates.get("github_event") {
            let event: TaskGithubEvent = serde_json::from_value(value.clone())
                .context("Failed to parse GitHub task update")?;
            task.timeline.push(TaskTimelineEntry {
                timestamp: now,
                kind: "github".to_string(),
                summary: format!(
                    "{} {}#{}: {}",
                    event.action, event.target, event.number, event.summary
                ),
                detail_path: None,
            });
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

fn load_state(
    tasks_dir: &Path,
    queue_path: &Path,
) -> Result<(HashMap<String, TaskRecord>, VecDeque<String>)> {
    let mut tasks = HashMap::new();
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
                task.status = TaskStatus::Queued;
                task.started_at = None;
                task.ended_at = None;
                task.duration_ms = None;
                task.timeline.push(TaskTimelineEntry {
                    timestamp: Utc::now(),
                    kind: "recovered".to_string(),
                    summary: "Recovered from restart and re-queued".to_string(),
                    detail_path: None,
                });
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

    Ok((tasks, queue))
}

fn resolve_task_id(tasks: &HashMap<String, TaskRecord>, id_or_prefix: &str) -> Result<String> {
    if tasks.contains_key(id_or_prefix) {
        return Ok(id_or_prefix.to_string());
    }
    let matches = tasks
        .keys()
        .filter(|id| id.starts_with(id_or_prefix))
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

/// Default task persistence location (`~/.deepseek/tasks`).
#[must_use]
pub fn default_tasks_dir() -> PathBuf {
    if let Ok(path) = std::env::var("DEEPSEEK_TASKS_DIR")
        && !path.trim().is_empty()
    {
        return PathBuf::from(path);
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".deepseek").join("tasks");
    }
    PathBuf::from(".deepseek").join("tasks")
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
    use std::fs;
    use tokio::time::Duration;

    struct MockExecutor;

    #[async_trait]
    impl TaskExecutor for MockExecutor {
        async fn execute(
            &self,
            task: ExecutionTask,
            events: mpsc::UnboundedSender<TaskExecutionEvent>,
            cancel: CancellationToken,
        ) -> TaskExecutionResult {
            let _ = events.send(TaskExecutionEvent::Status {
                message: format!("running {}", task.id),
            });
            let _ = events.send(TaskExecutionEvent::ThreadLinked {
                thread_id: "thr_test".to_string(),
                turn_id: "turn_test".to_string(),
            });
            let _ = events.send(TaskExecutionEvent::ToolStarted {
                id: "tool_1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "README.md" }),
            });
            sleep(Duration::from_millis(50)).await;
            if cancel.is_cancelled() {
                return TaskExecutionResult {
                    status: TaskStatus::Canceled,
                    result_text: None,
                    error: None,
                };
            }
            let _ = events.send(TaskExecutionEvent::ToolCompleted {
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
            });
            TaskExecutionResult {
                status: TaskStatus::Completed,
                result_text: Some("done".to_string()),
                error: None,
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
            max_subagents: 2,
        }
    }

    #[tokio::test]
    async fn persists_and_recovers_task_records() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test persistence"))
            .await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Completed);
        assert_eq!(finished.thread_id.as_deref(), Some("thr_test"));
        assert_eq!(finished.turn_id.as_deref(), Some("turn_test"));
        assert_eq!(finished.checklist.items.len(), 1);
        assert_eq!(finished.checklist.in_progress_id, Some(1));

        drop(manager);

        let recovered =
            TaskManager::start_with_executor(test_config(root.clone()), Arc::new(MockExecutor))
                .await?;
        let loaded = recovered.get_task(&task.id).await?;
        assert_eq!(loaded.status, TaskStatus::Completed);
        assert!(!loaded.timeline.is_empty());
        assert_eq!(loaded.checklist.items[0].content, "read fixture");
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
                            "command": "cargo test -p deepseek-tui --lib",
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
    async fn cancel_running_task_marks_canceled() -> Result<()> {
        let root = std::env::temp_dir().join(format!("deepseek-task-test-{}", Uuid::new_v4()));
        let manager =
            TaskManager::start_with_executor(test_config(root), Arc::new(MockExecutor)).await?;

        let task = manager
            .add_task(NewTaskRequest::from_prompt("test cancellation"))
            .await?;

        sleep(Duration::from_millis(10)).await;
        let _ = manager.cancel_task(&task.id).await?;
        let finished = wait_for_terminal_state(&manager, &task.id, Duration::from_secs(10)).await?;
        assert_eq!(finished.status, TaskStatus::Canceled);
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
}
