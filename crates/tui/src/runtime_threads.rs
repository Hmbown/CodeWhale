//! Durable thread/turn/item runtime for the HTTP API and background tasks.
//!
//! This module keeps DeepSeek-only execution while exposing Codex-like lifecycle
//! semantics (threads, turns, items, interrupt/steer, and replayable events).

// Background-task runtime — runs alongside the TUI. Raw stdio prints
// here would still land in the alt-screen on whichever terminal the
// foreground TUI happens to own. Route everything through `tracing::*`
// instead — see `runtime_log` for the rationale.
#![deny(clippy::print_stdout)]
#![deny(clippy::print_stderr)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, broadcast, oneshot};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::compaction::CompactionConfig;
use crate::config::{Config, DEFAULT_TEXT_MODEL, MAX_SUBAGENTS};
use crate::core::coherence::CoherenceState;
use crate::core::engine::{EngineConfig, EngineHandle, spawn_engine};
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use crate::core::ops::Op;
use crate::models::{ContentBlock, Message, SystemPrompt, Usage, compaction_threshold_for_model};
use crate::tools::plan::new_shared_plan_state;
use crate::tools::subagent::SubAgentStatus;
use crate::tools::todo::new_shared_todo_list;
use crate::tui::app::AppMode;

const EVENT_CHANNEL_CAPACITY: usize = 1024;
const MAX_ACTIVE_THREADS_DEFAULT: usize = 8;
const SUMMARY_LIMIT: usize = 280;

fn validated_record_id<'a>(id: &'a str, label: &str) -> Result<&'a str> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        bail!("{label} cannot be empty");
    }
    if trimmed != id {
        bail!("{label} cannot contain leading or trailing whitespace");
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("{label} contains unsupported characters");
    }
    Ok(trimmed)
}

/// Bumped to 2 for v0.6.6 — see issue #124. The persisted thread/turn/item
/// records didn't change shape, but the live engine semantics did: cycle
/// boundaries advance the `Session.cycle_count` and produce archived JSONL
/// files at `~/.deepseek/sessions/<id>/cycles/<n>.jsonl`. A v1 reader on a
/// session written by v2 wouldn't know about the cycle archive directory and
/// might misinterpret message counts; bumping is the safe choice.
const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 2;
const RUNTIME_RESTART_REASON: &str = "Interrupted by process restart";
const APPROVAL_DECISION_TIMEOUT: Duration = Duration::from_secs(300);

const fn default_runtime_schema_version() -> u32 {
    CURRENT_RUNTIME_SCHEMA_VERSION
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTurnStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemKind {
    UserMessage,
    AgentMessage,
    AgentReasoning,
    ToolCall,
    FileChange,
    CommandExecution,
    ContextCompaction,
    Status,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnItemLifecycleStatus {
    Queued,
    InProgress,
    Completed,
    Failed,
    Interrupted,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub model: String,
    pub workspace: PathBuf,
    pub mode: String,
    pub allow_shell: bool,
    pub trust_mode: bool,
    pub auto_approve: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_response_bookmark: Option<String>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// User-set title for the thread. When `None`, consumers fall back to a
    /// derived title (typically the latest turn's input summary). Added in
    /// v0.8.10 (#562); old runtime records simply have no `title` and behave
    /// as before. Schema version is not bumped because this field is purely
    /// additive metadata — older readers ignore it without misinterpretation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub coherence_state: CoherenceState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub thread_id: String,
    pub status: RuntimeTurnStatus,
    pub input_summary: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub item_ids: Vec<String>,
    #[serde(default)]
    pub steer_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnItemRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub id: String,
    pub turn_id: String,
    pub kind: TurnItemKind,
    pub status: TurnItemLifecycleStatus,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub artifact_refs: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeEventRecord {
    #[serde(default = "default_runtime_schema_version")]
    pub schema_version: u32,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    pub event: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeStoreState {
    #[serde(default = "default_runtime_schema_version")]
    schema_version: u32,
    next_seq: u64,
}

impl Default for RuntimeStoreState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            next_seq: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeThreadStore {
    threads_dir: PathBuf,
    turns_dir: PathBuf,
    items_dir: PathBuf,
    events_dir: PathBuf,
    state_path: PathBuf,
    state: Arc<Mutex<RuntimeStoreState>>,
}

impl RuntimeThreadStore {
    pub fn open(root: PathBuf) -> Result<Self> {
        let threads_dir = root.join("threads");
        let turns_dir = root.join("turns");
        let items_dir = root.join("items");
        let events_dir = root.join("events");
        fs::create_dir_all(&threads_dir)
            .with_context(|| format!("Failed to create {}", threads_dir.display()))?;
        fs::create_dir_all(&turns_dir)
            .with_context(|| format!("Failed to create {}", turns_dir.display()))?;
        fs::create_dir_all(&items_dir)
            .with_context(|| format!("Failed to create {}", items_dir.display()))?;
        fs::create_dir_all(&events_dir)
            .with_context(|| format!("Failed to create {}", events_dir.display()))?;

        let state_path = root.join("state.json");
        let state = if state_path.exists() {
            let raw = fs::read_to_string(&state_path)
                .with_context(|| format!("Failed to read {}", state_path.display()))?;
            serde_json::from_str::<RuntimeStoreState>(&raw)
                .with_context(|| format!("Failed to parse {}", state_path.display()))?
        } else {
            let default = RuntimeStoreState::default();
            write_json_atomic(&state_path, &default)?;
            default
        };

        Ok(Self {
            threads_dir,
            turns_dir,
            items_dir,
            events_dir,
            state_path,
            state: Arc::new(Mutex::new(state)),
        })
    }

    fn record_path(base: &Path, id: &str, extension: &str, label: &str) -> Result<PathBuf> {
        let id = validated_record_id(id, label)?;
        Ok(base.join(format!("{id}.{extension}")))
    }

    fn thread_path(&self, thread_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.threads_dir, thread_id, "json", "thread id")
    }

    fn turn_path(&self, turn_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.turns_dir, turn_id, "json", "turn id")
    }

    fn item_path(&self, item_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.items_dir, item_id, "json", "item id")
    }

    fn events_path(&self, thread_id: &str) -> Result<PathBuf> {
        Self::record_path(&self.events_dir, thread_id, "jsonl", "thread id")
    }

    pub fn save_thread(&self, thread: &ThreadRecord) -> Result<()> {
        write_json_atomic(&self.thread_path(&thread.id)?, thread)
    }

    pub fn save_turn(&self, turn: &TurnRecord) -> Result<()> {
        validated_record_id(&turn.thread_id, "thread id")?;
        write_json_atomic(&self.turn_path(&turn.id)?, turn)
    }

    pub fn save_item(&self, item: &TurnItemRecord) -> Result<()> {
        validated_record_id(&item.turn_id, "turn id")?;
        write_json_atomic(&self.item_path(&item.id)?, item)
    }

    pub fn load_thread(&self, thread_id: &str) -> Result<ThreadRecord> {
        let path = self.thread_path(thread_id)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read thread {}", path.display()))?;
        let record: ThreadRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse thread {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Thread schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn load_turn(&self, turn_id: &str) -> Result<TurnRecord> {
        let path = self.turn_path(turn_id)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read turn {}", path.display()))?;
        let record: TurnRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse turn {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Turn schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn load_item(&self, item_id: &str) -> Result<TurnItemRecord> {
        let path = self.item_path(item_id)?;
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read item {}", path.display()))?;
        let record: TurnItemRecord = serde_json::from_str(&raw)
            .with_context(|| format!("Failed to parse item {}", path.display()))?;
        if record.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
            bail!(
                "Item schema v{} is newer than supported v{}",
                record.schema_version,
                CURRENT_RUNTIME_SCHEMA_VERSION
            );
        }
        Ok(record)
    }

    pub fn list_threads(&self) -> Result<Vec<ThreadRecord>> {
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.threads_dir)
            .with_context(|| format!("Failed to read {}", self.threads_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let thread: ThreadRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if thread.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Thread schema v{} is newer than supported v{}",
                    thread.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            out.push(thread);
        }
        out.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
        Ok(out)
    }

    pub fn list_turns_for_thread(&self, thread_id: &str) -> Result<Vec<TurnRecord>> {
        validated_record_id(thread_id, "thread id")?;
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.turns_dir)
            .with_context(|| format!("Failed to read {}", self.turns_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let turn: TurnRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if turn.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Turn schema v{} is newer than supported v{}",
                    turn.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            if turn.thread_id == thread_id {
                out.push(turn);
            }
        }
        out.sort_by_key(|a| a.created_at);
        Ok(out)
    }

    pub fn list_items_for_turn(&self, turn_id: &str) -> Result<Vec<TurnItemRecord>> {
        validated_record_id(turn_id, "turn id")?;
        let mut out = Vec::new();
        for entry in fs::read_dir(&self.items_dir)
            .with_context(|| format!("Failed to read {}", self.items_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let item: TurnItemRecord = serde_json::from_str(&raw)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if item.schema_version > CURRENT_RUNTIME_SCHEMA_VERSION {
                bail!(
                    "Item schema v{} is newer than supported v{}",
                    item.schema_version,
                    CURRENT_RUNTIME_SCHEMA_VERSION
                );
            }
            if item.turn_id == turn_id {
                out.push(item);
            }
        }
        out.sort_by(|a, b| {
            let left = a.started_at.unwrap_or_else(Utc::now);
            let right = b.started_at.unwrap_or_else(Utc::now);
            left.cmp(&right)
        });
        Ok(out)
    }

    pub async fn append_event(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        item_id: Option<&str>,
        event: impl Into<String>,
        payload: Value,
    ) -> Result<RuntimeEventRecord> {
        validated_record_id(thread_id, "thread id")?;
        if let Some(turn_id) = turn_id {
            validated_record_id(turn_id, "turn id")?;
        }
        if let Some(item_id) = item_id {
            validated_record_id(item_id, "item id")?;
        }
        let path = self.events_path(thread_id)?;

        let mut state = self.state.lock().await;
        let seq = state.next_seq;
        state.next_seq = state.next_seq.saturating_add(1);
        write_json_atomic(&self.state_path, &*state)?;
        drop(state);

        let record = RuntimeEventRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            seq,
            timestamp: Utc::now(),
            thread_id: thread_id.to_string(),
            turn_id: turn_id.map(ToString::to_string),
            item_id: item_id.map(ToString::to_string),
            event: event.into(),
            payload,
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("Failed to open {}", path.display()))?;
        let line = serde_json::to_string(&record)?;
        writeln!(file, "{line}").with_context(|| format!("Failed to append {}", path.display()))?;
        file.flush()
            .with_context(|| format!("Failed to flush {}", path.display()))?;
        file.sync_all()
            .with_context(|| format!("Failed to fsync {}", path.display()))?;
        Ok(record)
    }

    pub fn events_since(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
    ) -> Result<Vec<RuntimeEventRecord>> {
        let path = self.events_path(thread_id)?;
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file =
            File::open(&path).with_context(|| format!("Failed to open {}", path.display()))?;
        let reader = BufReader::new(file);
        let mut out = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: RuntimeEventRecord = serde_json::from_str(&line)
                .with_context(|| format!("Failed to parse event line in {}", path.display()))?;
            if let Some(since) = since_seq
                && event.seq <= since
            {
                continue;
            }
            out.push(event);
        }
        Ok(out)
    }

    pub async fn current_seq(&self) -> u64 {
        let state = self.state.lock().await;
        state.next_seq.saturating_sub(1)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeThreadManagerConfig {
    pub data_dir: PathBuf,
    pub task_data_dir: PathBuf,
    pub max_active_threads: usize,
}

impl RuntimeThreadManagerConfig {
    #[must_use]
    pub fn from_task_data_dir(task_data_dir: PathBuf) -> Self {
        let data_dir = if let Ok(override_dir) = std::env::var("DEEPSEEK_RUNTIME_DIR") {
            if override_dir.trim().is_empty() {
                task_data_dir.join("runtime")
            } else {
                PathBuf::from(override_dir)
            }
        } else {
            task_data_dir.join("runtime")
        };
        Self {
            data_dir,
            task_data_dir,
            max_active_threads: MAX_ACTIVE_THREADS_DEFAULT,
        }
    }
}

/// Visibility filter for `list_threads`. Default is `ActiveOnly`. The runtime
/// API exposes this as the combination of `include_archived` and
/// `archived_only` query params (see `runtime_api.rs`); whalescale#260 / #563.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThreadListFilter {
    /// Only `archived = false` threads. The original default.
    #[default]
    ActiveOnly,
    /// Active and archived threads, sorted as the store returns them.
    IncludeArchived,
    /// Only `archived = true` threads.
    ArchivedOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateThreadRequest {
    pub model: Option<String>,
    pub workspace: Option<PathBuf>,
    pub mode: Option<String>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub task_id: Option<String>,
}

/// Mutable fields accepted by `PATCH /v1/threads/{id}`.
///
/// Each field is optional — missing means "no change". Extended in v0.8.10
/// (#562, whalescale#256) so the UI can flip persistent thread state without
/// having to recreate a thread or pass per-turn overrides on every send.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateThreadRequest {
    pub archived: Option<bool>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub title: Option<String>,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartTurnRequest {
    pub prompt: String,
    #[serde(default)]
    pub input_summary: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerTurnRequest {
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompactThreadRequest {
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadDetail {
    pub thread: ThreadRecord,
    pub turns: Vec<TurnRecord>,
    pub items: Vec<TurnItemRecord>,
    pub latest_seq: u64,
}

/// Aggregation key for `aggregate_usage`. Whalescale#261 / #564.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageGroupBy {
    Day,
    Model,
    Provider,
    Thread,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageBucket {
    pub key: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
    pub cost_usd: f64,
    pub turns: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsageAggregation {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub group_by: String,
    pub totals: UsageTotals,
    pub buckets: Vec<UsageBucket>,
}

/// Best-effort provider classification from a model name. Used as a grouping
/// key for `/v1/usage?group_by=provider`. Cost-tracking already runs the
/// model→pricing→cost path; this only labels the bucket.
fn provider_label_for_model(model: &str) -> &'static str {
    if model.starts_with("deepseek-ai/") {
        "nvidia-nim"
    } else if model.starts_with("deepseek-") {
        "deepseek"
    } else if model.starts_with("openai/") || model.starts_with("anthropic/") {
        "openrouter"
    } else {
        "unknown"
    }
}

#[derive(Debug, Clone)]
struct ActiveTurnState {
    turn_id: String,
    interrupt_requested: bool,
    auto_approve: bool,
    trust_mode: bool,
}

#[derive(Clone)]
struct ActiveThreadState {
    engine: EngineHandle,
    active_turn: Option<ActiveTurnState>,
}

#[derive(Default)]
struct ActiveThreads {
    engines: HashMap<String, ActiveThreadState>,
    lru: VecDeque<String>,
}

pub type SharedRuntimeThreadManager = Arc<RuntimeThreadManager>;

/// Manages active engine threads, lifecycle, and event persistence.
///
/// # Lock ordering invariant
///
/// Two `Mutex`es exist across this module:
/// - `RuntimeThreadStore::state` — protects the monotonic event sequence counter.
/// - `RuntimeThreadManager::active` — protects the set of loaded engine handles.
///
/// **No code path holds both locks simultaneously.** The `state` lock is only
/// acquired inside `RuntimeThreadStore::append_event` (where it is explicitly
/// dropped before any I/O) and `current_seq`. All `emit_event` calls (which
/// call `append_event`) happen *after* `active` has been released. If you add
/// new code that touches both, always acquire `state` before `active` to
/// preserve a consistent ordering.
#[derive(Clone)]
pub struct RuntimeThreadManager {
    config: Config,
    workspace: PathBuf,
    store: RuntimeThreadStore,
    active: Arc<Mutex<ActiveThreads>>,
    event_tx: broadcast::Sender<RuntimeEventRecord>,
    manager_cfg: RuntimeThreadManagerConfig,
    cancel_token: CancellationToken,
    task_manager: Arc<StdMutex<Option<crate::task_manager::SharedTaskManager>>>,
    automations: Arc<StdMutex<Option<crate::automation_manager::SharedAutomationManager>>>,
    pending_approvals: Arc<StdMutex<HashMap<String, oneshot::Sender<ExternalApprovalDecision>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeApprovalDecision {
    ApproveTool,
    DenyTool,
    RetryWithFullAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalApprovalDecision {
    Allow { remember: bool },
    Deny { remember: bool },
}

impl RuntimeThreadManager {
    pub fn open(
        config: Config,
        workspace: PathBuf,
        manager_cfg: RuntimeThreadManagerConfig,
    ) -> Result<Self> {
        let store = RuntimeThreadStore::open(manager_cfg.data_dir.clone())?;
        let (event_tx, _event_rx) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let manager = Self {
            config,
            workspace,
            store,
            active: Arc::new(Mutex::new(ActiveThreads::default())),
            event_tx,
            manager_cfg,
            cancel_token: CancellationToken::new(),
            task_manager: Arc::new(StdMutex::new(None)),
            automations: Arc::new(StdMutex::new(None)),
            pending_approvals: Arc::new(StdMutex::new(HashMap::new())),
        };
        manager.recover_interrupted_state()?;
        Ok(manager)
    }

    /// Attach the durable task manager so model-visible task tools work inside
    /// runtime thread turns as well as interactive TUI turns.
    pub fn attach_task_manager(&self, task_manager: crate::task_manager::SharedTaskManager) {
        if let Ok(mut slot) = self.task_manager.lock() {
            *slot = Some(task_manager);
        }
    }

    /// Attach the automation manager for model-visible scheduling tools.
    pub fn attach_automation_manager(
        &self,
        automations: crate::automation_manager::SharedAutomationManager,
    ) {
        if let Ok(mut slot) = self.automations.lock() {
            *slot = Some(automations);
        }
    }

    #[allow(dead_code)] // Public API for external callers (runtime API, task manager)
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
        if let Ok(mut map) = self.pending_approvals.lock() {
            map.clear();
        }
    }

    #[allow(dead_code)] // Public API for external callers
    pub fn is_shutdown(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    fn register_pending_approval(
        &self,
        approval_id: &str,
    ) -> oneshot::Receiver<ExternalApprovalDecision> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending_approvals.lock() {
            map.insert(approval_id.to_string(), tx);
        }
        rx
    }

    fn cancel_pending_approval(&self, approval_id: &str) {
        if let Ok(mut map) = self.pending_approvals.lock() {
            map.remove(approval_id);
        }
    }

    pub fn deliver_external_approval(
        &self,
        approval_id: &str,
        decision: ExternalApprovalDecision,
    ) -> bool {
        let sender = match self.pending_approvals.lock() {
            Ok(mut map) => map.remove(approval_id),
            Err(_) => return false,
        };
        match sender {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    #[allow(dead_code)]
    pub fn pending_approvals_count(&self) -> usize {
        self.pending_approvals
            .lock()
            .map(|map| map.len())
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub(crate) fn register_pending_approval_for_test(
        &self,
        approval_id: &str,
    ) -> oneshot::Receiver<ExternalApprovalDecision> {
        self.register_pending_approval(approval_id)
    }

    async fn remember_thread_auto_approve(&self, thread_id: &str) {
        let Ok(mut thread) = self.store.load_thread(thread_id) else {
            return;
        };
        if thread.auto_approve {
            return;
        }
        thread.auto_approve = true;
        thread.updated_at = Utc::now();
        if let Err(err) = self.store.save_thread(&thread) {
            tracing::warn!(
                "Failed to persist auto_approve flip for thread {}: {}",
                thread_id,
                err
            );
        }
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<RuntimeEventRecord> {
        self.event_tx.subscribe()
    }

    async fn emit_event(
        &self,
        thread_id: &str,
        turn_id: Option<&str>,
        item_id: Option<&str>,
        event: impl Into<String>,
        payload: Value,
    ) -> Result<RuntimeEventRecord> {
        let record = self
            .store
            .append_event(thread_id, turn_id, item_id, event, payload)
            .await?;
        if let Err(e) = self.event_tx.send(record.clone()) {
            tracing::debug!(
                "Runtime event broadcast failed (no receivers or channel full): {}",
                e
            );
        }
        Ok(record)
    }

    pub async fn create_thread(&self, req: CreateThreadRequest) -> Result<ThreadRecord> {
        let now = Utc::now();
        let model = req
            .model
            .filter(|m| !m.trim().is_empty())
            .or_else(|| self.config.default_text_model.clone())
            .unwrap_or_else(|| DEFAULT_TEXT_MODEL.to_string());
        let workspace = req.workspace.unwrap_or_else(|| self.workspace.clone());
        let mode = req
            .mode
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "agent".to_string());
        let allow_shell = req.allow_shell.unwrap_or_else(|| self.config.allow_shell());
        let trust_mode = req.trust_mode.unwrap_or(false);
        let auto_approve = req.auto_approve.unwrap_or(false);

        let thread = ThreadRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: format!("thr_{}", &Uuid::new_v4().to_string()[..8]),
            created_at: now,
            updated_at: now,
            model,
            workspace,
            mode,
            allow_shell,
            trust_mode,
            auto_approve,
            latest_turn_id: None,
            latest_response_bookmark: None,
            archived: req.archived,
            system_prompt: req.system_prompt,
            task_id: req.task_id,
            title: None,
            coherence_state: CoherenceState::default(),
        };
        self.store.save_thread(&thread)?;
        self.emit_event(
            &thread.id,
            None,
            None,
            "thread.started",
            json!({ "thread": thread }),
        )
        .await?;
        Ok(thread)
    }

    pub async fn list_threads(
        &self,
        filter: ThreadListFilter,
        limit: Option<usize>,
    ) -> Result<Vec<ThreadRecord>> {
        let mut threads = self.store.list_threads()?;
        match filter {
            ThreadListFilter::ActiveOnly => threads.retain(|t| !t.archived),
            ThreadListFilter::ArchivedOnly => threads.retain(|t| t.archived),
            ThreadListFilter::IncludeArchived => {}
        }
        if let Some(limit) = limit {
            threads.truncate(limit);
        }
        Ok(threads)
    }

    /// Aggregate token + cost usage across all threads/turns inside the time
    /// range `[since, until]`. Each turn's cost is computed via
    /// `pricing::calculate_turn_cost_from_usage` using the *thread*'s model
    /// (turns inherit it). Whalescale#261 / #564.
    ///
    /// Buckets are sorted by ascending key for deterministic output. Empty
    /// ranges produce empty `buckets` (never an error).
    pub async fn aggregate_usage(
        &self,
        since: Option<DateTime<Utc>>,
        until: Option<DateTime<Utc>>,
        group_by: UsageGroupBy,
    ) -> Result<UsageAggregation> {
        use std::collections::BTreeMap;

        let mut buckets: BTreeMap<String, UsageBucket> = BTreeMap::new();
        let mut totals = UsageTotals::default();

        for thread in self.store.list_threads()? {
            let turns = self.store.list_turns_for_thread(&thread.id)?;
            for turn in turns {
                if let Some(s) = since
                    && turn.created_at < s
                {
                    continue;
                }
                if let Some(u) = until
                    && turn.created_at > u
                {
                    continue;
                }
                let Some(usage) = turn.usage.as_ref() else {
                    continue;
                };
                let cached = usage.prompt_cache_hit_tokens.unwrap_or(0) as u64;
                let reasoning = usage.reasoning_tokens.unwrap_or(0) as u64;
                let input = usage.input_tokens as u64;
                let output = usage.output_tokens as u64;
                let cost = crate::pricing::calculate_turn_cost_from_usage(&thread.model, usage)
                    .unwrap_or(0.0);

                totals.input_tokens += input;
                totals.output_tokens += output;
                totals.cached_tokens += cached;
                totals.reasoning_tokens += reasoning;
                totals.cost_usd += cost;
                totals.turns += 1;

                let key = match group_by {
                    UsageGroupBy::Day => turn.created_at.format("%Y-%m-%d").to_string(),
                    UsageGroupBy::Model => thread.model.clone(),
                    UsageGroupBy::Provider => provider_label_for_model(&thread.model).to_string(),
                    UsageGroupBy::Thread => thread.id.clone(),
                };
                let bucket = buckets.entry(key.clone()).or_insert_with(|| UsageBucket {
                    key,
                    ..UsageBucket::default()
                });
                bucket.input_tokens += input;
                bucket.output_tokens += output;
                bucket.cached_tokens += cached;
                bucket.reasoning_tokens += reasoning;
                bucket.cost_usd += cost;
                bucket.turns += 1;
            }
        }

        let group_by_str = match group_by {
            UsageGroupBy::Day => "day",
            UsageGroupBy::Model => "model",
            UsageGroupBy::Provider => "provider",
            UsageGroupBy::Thread => "thread",
        }
        .to_string();

        Ok(UsageAggregation {
            since,
            until,
            group_by: group_by_str,
            totals,
            buckets: buckets.into_values().collect(),
        })
    }

    pub async fn get_thread(&self, id: &str) -> Result<ThreadRecord> {
        self.store
            .load_thread(id)
            .with_context(|| format!("Thread not found: {id}"))
    }

    pub async fn update_thread(&self, id: &str, req: UpdateThreadRequest) -> Result<ThreadRecord> {
        if req.archived.is_none()
            && req.allow_shell.is_none()
            && req.trust_mode.is_none()
            && req.auto_approve.is_none()
            && req.model.is_none()
            && req.mode.is_none()
            && req.title.is_none()
            && req.system_prompt.is_none()
        {
            bail!("At least one thread field is required");
        }

        if let Some(model) = req.model.as_ref()
            && model.trim().is_empty()
        {
            bail!("model must not be empty");
        }
        if let Some(mode) = req.mode.as_ref()
            && mode.trim().is_empty()
        {
            bail!("mode must not be empty");
        }

        let mut thread = self.get_thread(id).await?;
        let mut changes = serde_json::Map::new();

        if let Some(archived) = req.archived
            && thread.archived != archived
        {
            thread.archived = archived;
            changes.insert("archived".to_string(), json!(archived));
        }
        if let Some(allow_shell) = req.allow_shell
            && thread.allow_shell != allow_shell
        {
            thread.allow_shell = allow_shell;
            changes.insert("allow_shell".to_string(), json!(allow_shell));
        }
        if let Some(trust_mode) = req.trust_mode
            && thread.trust_mode != trust_mode
        {
            thread.trust_mode = trust_mode;
            changes.insert("trust_mode".to_string(), json!(trust_mode));
        }
        if let Some(auto_approve) = req.auto_approve
            && thread.auto_approve != auto_approve
        {
            thread.auto_approve = auto_approve;
            changes.insert("auto_approve".to_string(), json!(auto_approve));
        }
        if let Some(model) = req.model
            && thread.model != model
        {
            thread.model = model.clone();
            changes.insert("model".to_string(), json!(model));
        }
        if let Some(mode) = req.mode
            && thread.mode != mode
        {
            thread.mode = mode.clone();
            changes.insert("mode".to_string(), json!(mode));
        }
        if let Some(title) = req.title {
            // Empty string clears a previously-set title and reverts to derived.
            let new_title = if title.trim().is_empty() {
                None
            } else {
                Some(title)
            };
            if thread.title != new_title {
                thread.title = new_title.clone();
                changes.insert("title".to_string(), json!(new_title));
            }
        }
        if let Some(system_prompt) = req.system_prompt {
            let new_sys = if system_prompt.trim().is_empty() {
                None
            } else {
                Some(system_prompt)
            };
            if thread.system_prompt != new_sys {
                thread.system_prompt = new_sys.clone();
                changes.insert("system_prompt".to_string(), json!(new_sys));
            }
        }

        if !changes.is_empty() {
            thread.updated_at = Utc::now();
            self.store.save_thread(&thread)?;
            self.emit_event(
                &thread.id,
                None,
                None,
                "thread.updated",
                json!({
                    "thread": thread.clone(),
                    "changes": Value::Object(changes),
                }),
            )
            .await?;
        }

        Ok(thread)
    }

    pub async fn get_thread_detail(&self, id: &str) -> Result<ThreadDetail> {
        let thread = self.get_thread(id).await?;
        let turns = self.store.list_turns_for_thread(id)?;
        let mut items = Vec::new();
        for turn in &turns {
            items.extend(self.store.list_items_for_turn(&turn.id)?);
        }
        let latest_seq = self.store.current_seq().await;
        Ok(ThreadDetail {
            thread,
            turns,
            items,
            latest_seq,
        })
    }

    pub async fn resume_thread(&self, id: &str) -> Result<ThreadRecord> {
        let thread = self.get_thread(id).await?;
        self.ensure_engine_loaded(&thread).await?;
        Ok(thread)
    }

    /// Resume a thread and recover the sub-agent rebind hints needed to
    /// reconstruct in-transcript cards (issue #128). Drains the persisted
    /// `agent.*` event stream and collapses it into the latest known
    /// status per `agent_id` — the UI consumes this to seed empty
    /// `DelegateCard` / `FanoutCard` placeholders so subsequent live
    /// mailbox envelopes mutate them in place.
    #[allow(dead_code)] // exposed for the runtime API resume flow; consumed by #128 follow-up.
    pub async fn resume_thread_with_agent_rebind(
        &self,
        id: &str,
    ) -> Result<(ThreadRecord, Vec<AgentRebindHint>)> {
        let thread = self.resume_thread(id).await?;
        let events = self.store.events_since(&thread.id, None)?;
        let hints = collect_agent_rebind_hints(&events);
        Ok((thread, hints))
    }

    pub async fn fork_thread(&self, id: &str) -> Result<ThreadRecord> {
        let source = self.get_thread(id).await?;
        let mut forked = source.clone();
        let now = Utc::now();
        forked.id = format!("thr_{}", &Uuid::new_v4().to_string()[..8]);
        forked.created_at = now;
        forked.updated_at = now;
        forked.latest_turn_id = None;
        forked.archived = false;
        self.store.save_thread(&forked)?;

        let source_turns = self.store.list_turns_for_thread(&source.id)?;
        for source_turn in source_turns {
            let mut cloned_turn = source_turn.clone();
            cloned_turn.id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
            cloned_turn.thread_id = forked.id.clone();
            cloned_turn.item_ids.clear();
            self.store.save_turn(&cloned_turn)?;

            let items = self.store.list_items_for_turn(&source_turn.id)?;
            for item in items {
                let mut cloned_item = item.clone();
                cloned_item.id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                cloned_item.turn_id = cloned_turn.id.clone();
                self.store.save_item(&cloned_item)?;
                cloned_turn.item_ids.push(cloned_item.id.clone());
            }
            self.store.save_turn(&cloned_turn)?;
            forked.latest_turn_id = Some(cloned_turn.id.clone());
            forked.updated_at = now;
            self.store.save_thread(&forked)?;
        }

        self.emit_event(
            &forked.id,
            None,
            None,
            "thread.forked",
            json!({
                "thread": forked,
                "source_thread_id": source.id,
            }),
        )
        .await?;
        Ok(forked)
    }

    /// Fork a thread, dropping every turn from the Nth-from-tail user
    /// message onward (issue #133 — Esc-Esc backtrack).
    ///
    /// `depth_from_tail` selects which user turn to roll back *to*:
    ///
    /// - `0` — drop the most recent turn (the freshest user message and
    ///   everything after it)
    /// - `1` — drop the two most recent turns (rewind one further)
    /// - …and so on
    ///
    /// Returns a tuple of `(forked_thread, original_user_text)` where the
    /// second element is the `detail` of the first `UserMessage` item in
    /// the *first dropped* turn — i.e. the input the user typed to start
    /// that turn — so the caller can pre-populate the composer with it.
    /// `None` when no detail was recorded (defensive — every persisted
    /// `UserMessage` since v0.6 carries a detail string).
    ///
    /// Counts user turns by iterating `list_turns_for_thread` (sorted
    /// oldest → newest) backwards. A turn is counted as a "user turn"
    /// when at least one of its items has `kind ==
    /// TurnItemKind::UserMessage`. Steered turns (which append additional
    /// `UserMessage` items) still count as one turn — backtrack rewinds
    /// at the turn boundary, not at the steer boundary.
    ///
    /// Errors:
    /// - `depth_from_tail` exceeds the number of user turns
    /// - source thread not found
    #[allow(dead_code)] // exposed for the runtime/HTTP fork-on-backtrack path; the in-TUI Esc-Esc flow trims `App` state directly. Issue #133.
    pub async fn fork_at_user_message(
        &self,
        id: &str,
        depth_from_tail: usize,
    ) -> Result<(ThreadRecord, Option<String>)> {
        let source = self.get_thread(id).await?;
        let source_turns = self.store.list_turns_for_thread(&source.id)?;

        // Walk turns from newest to oldest. For each turn, ask: does it
        // contain a UserMessage item? If yes, it counts toward the depth.
        let mut user_turn_indices: Vec<usize> = Vec::new();
        for (idx, turn) in source_turns.iter().enumerate().rev() {
            let items = self.store.list_items_for_turn(&turn.id)?;
            if items
                .iter()
                .any(|item| item.kind == TurnItemKind::UserMessage)
            {
                user_turn_indices.push(idx);
            }
        }
        if depth_from_tail >= user_turn_indices.len() {
            bail!(
                "fork_at_user_message: depth {} exceeds {} user turn(s)",
                depth_from_tail,
                user_turn_indices.len()
            );
        }
        // `user_turn_indices` is newest-first because we iterated in
        // reverse, so the Nth element is exactly the Nth-from-tail user
        // turn in the original chronological list.
        let target_turn_idx = user_turn_indices[depth_from_tail];
        let target_turn_id = source_turns[target_turn_idx].id.clone();

        // Pull the original user-message text out of the dropped turn so
        // the caller can drop it back into the composer.
        let target_items = self.store.list_items_for_turn(&target_turn_id)?;
        let original_user_text = target_items
            .iter()
            .find(|item| item.kind == TurnItemKind::UserMessage)
            .and_then(|item| item.detail.clone());

        // Copy turns strictly before `target_turn_idx` into a new thread.
        // Mirrors `fork_thread` but stops at the cutoff instead of copying
        // every turn. Kept structurally close so future parity reviews
        // can spot drift between the two paths.
        let mut forked = source.clone();
        let now = Utc::now();
        forked.id = format!("thr_{}", &Uuid::new_v4().to_string()[..8]);
        forked.created_at = now;
        forked.updated_at = now;
        forked.latest_turn_id = None;
        forked.archived = false;
        self.store.save_thread(&forked)?;

        for source_turn in source_turns.iter().take(target_turn_idx) {
            let mut cloned_turn = source_turn.clone();
            cloned_turn.id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
            cloned_turn.thread_id = forked.id.clone();
            cloned_turn.item_ids.clear();
            self.store.save_turn(&cloned_turn)?;

            let items = self.store.list_items_for_turn(&source_turn.id)?;
            for item in items {
                let mut cloned_item = item.clone();
                cloned_item.id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                cloned_item.turn_id = cloned_turn.id.clone();
                self.store.save_item(&cloned_item)?;
                cloned_turn.item_ids.push(cloned_item.id.clone());
            }
            self.store.save_turn(&cloned_turn)?;
            forked.latest_turn_id = Some(cloned_turn.id.clone());
            forked.updated_at = now;
            self.store.save_thread(&forked)?;
        }

        self.emit_event(
            &forked.id,
            None,
            None,
            "thread.forked",
            json!({
                "thread": forked,
                "source_thread_id": source.id,
                "backtrack_depth_from_tail": depth_from_tail,
                "dropped_turn_id": target_turn_id,
            }),
        )
        .await?;
        Ok((forked, original_user_text))
    }

    /// Seed a thread with messages from a saved session so subsequent turns
    /// continue with the prior conversation context.
    pub async fn seed_thread_from_messages(
        &self,
        thread_id: &str,
        messages: &[Message],
    ) -> Result<()> {
        let mut thread = self.get_thread(thread_id).await?;
        let now = Utc::now();

        let mut user_buf: Vec<String> = Vec::new();
        let mut pending_pairs: Vec<(String, Option<String>)> = Vec::new();

        for msg in messages {
            let text = msg
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                continue;
            }
            if msg.role == "user" {
                user_buf.push(text);
            } else if msg.role == "assistant" {
                let user_text = if user_buf.is_empty() {
                    String::new()
                } else {
                    std::mem::take(&mut user_buf).join("\n")
                };
                pending_pairs.push((user_text, Some(text)));
            }
        }
        if !user_buf.is_empty() {
            let user_text = std::mem::take(&mut user_buf).join("\n");
            pending_pairs.push((user_text, None));
        }

        for (user_text, assistant_text) in pending_pairs {
            let turn_id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
            let summary = crate::utils::truncate_with_ellipsis(&user_text, SUMMARY_LIMIT, "...");
            let mut item_ids = Vec::new();

            if !user_text.is_empty() {
                let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                self.store.save_item(&TurnItemRecord {
                    schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                    id: item_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: TurnItemKind::UserMessage,
                    status: TurnItemLifecycleStatus::Completed,
                    summary: summary.clone(),
                    detail: Some(user_text),
                    metadata: None,
                    artifact_refs: Vec::new(),
                    started_at: Some(now),
                    ended_at: Some(now),
                })?;
                item_ids.push(item_id);
            }

            if let Some(assistant_text) = assistant_text {
                let asst_summary = if assistant_text.len() > SUMMARY_LIMIT {
                    format!("{}...", &assistant_text[..SUMMARY_LIMIT.saturating_sub(3)])
                } else {
                    assistant_text.clone()
                };
                let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                self.store.save_item(&TurnItemRecord {
                    schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                    id: item_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: TurnItemKind::AgentMessage,
                    status: TurnItemLifecycleStatus::Completed,
                    summary: asst_summary,
                    detail: Some(assistant_text),
                    metadata: None,
                    artifact_refs: Vec::new(),
                    started_at: Some(now),
                    ended_at: Some(now),
                })?;
                item_ids.push(item_id);
            }

            self.store.save_turn(&TurnRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                id: turn_id.clone(),
                thread_id: thread_id.to_string(),
                status: RuntimeTurnStatus::Completed,
                input_summary: summary,
                created_at: now,
                started_at: Some(now),
                ended_at: Some(now),
                duration_ms: Some(0),
                usage: None,
                error: None,
                item_ids,
                steer_count: 0,
            })?;

            thread.latest_turn_id = Some(turn_id);
            thread.updated_at = now;
        }

        self.store.save_thread(&thread)?;
        self.emit_event(
            thread_id,
            None,
            None,
            "thread.updated",
            json!({ "thread": thread, "reason": "session_resume" }),
        )
        .await?;
        Ok(())
    }

    pub async fn start_turn(&self, thread_id: &str, req: StartTurnRequest) -> Result<TurnRecord> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("prompt is required");
        }

        let mut thread = self.get_thread(thread_id).await?;
        let engine = self.ensure_engine_loaded(&thread).await?;

        {
            let active = self.active.lock().await;
            if let Some(active_thread) = active.engines.get(thread_id)
                && active_thread.active_turn.is_some()
            {
                bail!("Thread already has an active turn");
            }
        }

        let now = Utc::now();
        let turn_id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
        let mut turn = TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: turn_id.clone(),
            thread_id: thread_id.to_string(),
            status: RuntimeTurnStatus::InProgress,
            input_summary: req
                .input_summary
                .unwrap_or_else(|| summarize_text(&prompt, SUMMARY_LIMIT)),
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            duration_ms: None,
            usage: None,
            error: None,
            item_ids: Vec::new(),
            steer_count: 0,
        };

        let user_item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
        let user_item = TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: user_item_id.clone(),
            turn_id: turn_id.clone(),
            kind: TurnItemKind::UserMessage,
            status: TurnItemLifecycleStatus::Completed,
            summary: summarize_text(&prompt, SUMMARY_LIMIT),
            detail: Some(prompt.clone()),
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(now),
            ended_at: Some(now),
        };

        turn.item_ids.push(user_item_id.clone());
        self.store.save_item(&user_item)?;
        self.store.save_turn(&turn)?;

        thread.latest_turn_id = Some(turn_id.clone());
        thread.updated_at = now;
        self.store.save_thread(&thread)?;

        self.emit_event(
            thread_id,
            Some(&turn_id),
            None,
            "turn.started",
            json!({ "turn": turn.clone() }),
        )
        .await?;
        self.emit_event(
            thread_id,
            Some(&turn_id),
            Some(&user_item_id),
            "item.started",
            json!({ "item": user_item.clone() }),
        )
        .await?;
        self.emit_event(
            thread_id,
            Some(&turn_id),
            Some(&user_item_id),
            "item.completed",
            json!({ "item": user_item }),
        )
        .await?;

        {
            let mut active = self.active.lock().await;
            let Some(state) = active.engines.get_mut(thread_id) else {
                bail!("Thread engine not loaded");
            };
            state.active_turn = Some(ActiveTurnState {
                turn_id: turn_id.clone(),
                interrupt_requested: false,
                auto_approve: req.auto_approve.unwrap_or(thread.auto_approve),
                trust_mode: req.trust_mode.unwrap_or(thread.trust_mode),
            });
            touch_lru(&mut active.lru, thread_id);
        }

        let mode = parse_mode(req.mode.as_deref().unwrap_or(&thread.mode));
        let requested_model = req.model.unwrap_or_else(|| thread.model.clone());
        let auto_model = requested_model.trim().eq_ignore_ascii_case("auto");
        let (model, reasoning_effort) = if auto_model {
            let selection = crate::commands::resolve_auto_route_with_flash(
                &self.config,
                &prompt,
                "",
                "auto",
                "auto",
            )
            .await;
            (
                selection.model,
                selection
                    .reasoning_effort
                    .map(|effort| effort.as_setting().to_string()),
            )
        } else {
            (requested_model, None)
        };
        let allow_shell = req.allow_shell.unwrap_or(thread.allow_shell);
        let trust_mode = req.trust_mode.unwrap_or(thread.trust_mode);
        let auto_approve = req.auto_approve.unwrap_or(thread.auto_approve);

        engine
            .send(Op::SendMessage {
                content: prompt,
                mode,
                model: model.clone(),
                goal_objective: None,
                reasoning_effort,
                reasoning_effort_auto: auto_model,
                auto_model,
                allow_shell,
                trust_mode,
                auto_approve,
                translation_enabled: false,
                approval_mode: if auto_approve {
                    crate::tui::approval::ApprovalMode::Auto
                } else {
                    crate::tui::approval::ApprovalMode::Suggest
                },
            })
            .await
            .map_err(|e| anyhow!("Failed to start turn: {e}"))?;

        let manager = Arc::new(self.clone());
        let thread_id_owned = thread_id.to_string();
        let turn_id_owned = turn_id.clone();
        let engine_clone = engine.clone();
        let cancel_token = self.cancel_token.clone();
        tokio::spawn(async move {
            if cancel_token.is_cancelled() {
                tracing::debug!("Skipping turn monitor: shutdown requested");
                return;
            }
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(manager.monitor_turn(
                thread_id_owned,
                turn_id_owned,
                engine_clone,
            ))
            .catch_unwind()
            .await;
            match result {
                Ok(res) => {
                    if let Err(err) = res {
                        tracing::error!("Failed to monitor turn: {err}");
                    }
                }
                Err(panic_err) => {
                    if let Some(msg) = panic_err.downcast_ref::<&str>() {
                        tracing::error!("Turn monitor panicked: {}", msg);
                    } else if let Some(msg) = panic_err.downcast_ref::<String>() {
                        tracing::error!("Turn monitor panicked: {}", msg);
                    } else {
                        tracing::error!("Turn monitor panicked with unknown error");
                    }
                }
            }
        });

        Ok(turn)
    }

    pub async fn interrupt_turn(&self, thread_id: &str, turn_id: &str) -> Result<TurnRecord> {
        {
            let mut active = self.active.lock().await;
            let Some(active_thread) = active.engines.get_mut(thread_id) else {
                bail!("Thread is not loaded");
            };
            let Some(active_turn) = active_thread.active_turn.as_mut() else {
                bail!("No active turn on thread {thread_id}");
            };
            if active_turn.turn_id != turn_id {
                bail!("Turn {turn_id} is not active on thread {thread_id}");
            }
            active_turn.interrupt_requested = true;
            active_thread.engine.cancel();
            touch_lru(&mut active.lru, thread_id);
        }

        self.emit_event(
            thread_id,
            Some(turn_id),
            None,
            "turn.interrupt_requested",
            json!({ "thread_id": thread_id, "turn_id": turn_id }),
        )
        .await?;

        self.store.load_turn(turn_id)
    }

    pub async fn steer_turn(
        &self,
        thread_id: &str,
        turn_id: &str,
        req: SteerTurnRequest,
    ) -> Result<TurnRecord> {
        let prompt = req.prompt.trim().to_string();
        if prompt.is_empty() {
            bail!("prompt is required");
        }

        let engine = {
            let mut active = self.active.lock().await;
            let engine = {
                let Some(active_thread) = active.engines.get_mut(thread_id) else {
                    bail!("Thread is not loaded");
                };
                let Some(active_turn) = active_thread.active_turn.as_mut() else {
                    bail!("No active turn on thread {thread_id}");
                };
                if active_turn.turn_id != turn_id {
                    bail!("Turn {turn_id} is not active on thread {thread_id}");
                }
                active_thread.engine.clone()
            };
            touch_lru(&mut active.lru, thread_id);
            engine
        };

        engine
            .steer(prompt.clone())
            .await
            .map_err(|e| anyhow!("Failed to steer turn: {e}"))?;

        let now = Utc::now();
        let mut turn = self.store.load_turn(turn_id)?;
        turn.steer_count = turn.steer_count.saturating_add(1);
        self.store.save_turn(&turn)?;

        let item = TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
            turn_id: turn_id.to_string(),
            kind: TurnItemKind::UserMessage,
            status: TurnItemLifecycleStatus::Completed,
            summary: summarize_text(&prompt, SUMMARY_LIMIT),
            detail: Some(prompt.clone()),
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(now),
            ended_at: Some(now),
        };
        turn.item_ids.push(item.id.clone());
        self.store.save_item(&item)?;
        self.store.save_turn(&turn)?;

        self.emit_event(
            thread_id,
            Some(turn_id),
            Some(&item.id),
            "turn.steered",
            json!({
                "thread_id": thread_id,
                "turn_id": turn_id,
                "input": prompt,
            }),
        )
        .await?;
        self.emit_event(
            thread_id,
            Some(turn_id),
            Some(&item.id),
            "item.completed",
            json!({ "item": item }),
        )
        .await?;

        Ok(turn)
    }

    pub async fn compact_thread(
        &self,
        thread_id: &str,
        req: CompactThreadRequest,
    ) -> Result<TurnRecord> {
        let mut thread = self.get_thread(thread_id).await?;
        let engine = self.ensure_engine_loaded(&thread).await?;

        {
            let active = self.active.lock().await;
            if let Some(active_thread) = active.engines.get(thread_id)
                && active_thread.active_turn.is_some()
            {
                bail!("Thread already has an active turn");
            }
        }

        let now = Utc::now();
        let turn_id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
        let turn = TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: turn_id.clone(),
            thread_id: thread_id.to_string(),
            status: RuntimeTurnStatus::InProgress,
            input_summary: req
                .reason
                .as_deref()
                .map(|s| summarize_text(s, SUMMARY_LIMIT))
                .unwrap_or_else(|| "Manual context compaction".to_string()),
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            duration_ms: None,
            usage: None,
            error: None,
            item_ids: Vec::new(),
            steer_count: 0,
        };
        self.store.save_turn(&turn)?;

        thread.latest_turn_id = Some(turn_id.clone());
        thread.updated_at = now;
        self.store.save_thread(&thread)?;

        {
            let mut active = self.active.lock().await;
            let Some(state) = active.engines.get_mut(thread_id) else {
                bail!("Thread engine not loaded");
            };
            state.active_turn = Some(ActiveTurnState {
                turn_id: turn_id.clone(),
                interrupt_requested: false,
                auto_approve: thread.auto_approve,
                trust_mode: thread.trust_mode,
            });
            touch_lru(&mut active.lru, thread_id);
        }

        self.emit_event(
            thread_id,
            Some(&turn_id),
            None,
            "turn.started",
            json!({ "turn": turn.clone(), "manual_compaction": true }),
        )
        .await?;

        engine
            .send(Op::CompactContext)
            .await
            .map_err(|e| anyhow!("Failed to trigger compaction: {e}"))?;

        let manager = Arc::new(self.clone());
        let thread_id_owned = thread_id.to_string();
        let turn_id_owned = turn_id.clone();
        let engine_clone = engine.clone();
        let cancel_token = self.cancel_token.clone();
        tokio::spawn(async move {
            if cancel_token.is_cancelled() {
                tracing::debug!("Skipping compaction monitor: shutdown requested");
                return;
            }
            use futures_util::FutureExt;
            let result = std::panic::AssertUnwindSafe(manager.monitor_turn(
                thread_id_owned,
                turn_id_owned,
                engine_clone,
            ))
            .catch_unwind()
            .await;
            match result {
                Ok(res) => {
                    if let Err(err) = res {
                        tracing::error!("Failed to monitor compaction turn: {err}");
                    }
                }
                Err(panic_err) => {
                    if let Some(msg) = panic_err.downcast_ref::<&str>() {
                        tracing::error!("Compaction monitor panicked: {}", msg);
                    } else if let Some(msg) = panic_err.downcast_ref::<String>() {
                        tracing::error!("Compaction monitor panicked: {}", msg);
                    } else {
                        tracing::error!("Compaction monitor panicked with unknown error");
                    }
                }
            }
        });

        Ok(turn)
    }

    pub fn events_since(
        &self,
        thread_id: &str,
        since_seq: Option<u64>,
    ) -> Result<Vec<RuntimeEventRecord>> {
        self.store.events_since(thread_id, since_seq)
    }

    async fn ensure_engine_loaded(&self, thread: &ThreadRecord) -> Result<EngineHandle> {
        {
            let mut active = self.active.lock().await;
            if let Some(engine) = active
                .engines
                .get(thread.id.as_str())
                .map(|state| state.engine.clone())
            {
                touch_lru(&mut active.lru, &thread.id);
                return Ok(engine);
            }
        }

        // Compaction defaults to disabled in v0.6.6 — the cycle architecture
        // (issue #124) handles long-context resets. Threads keep the
        // legacy summarizer wired off unless an operator opts in via config.
        let compaction = CompactionConfig {
            enabled: false,
            model: thread.model.clone(),
            token_threshold: compaction_threshold_for_model(&thread.model),
            ..Default::default()
        };
        let network_policy = self.config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        });
        let lsp_config = self
            .config
            .lsp
            .clone()
            .map(crate::config::LspConfigToml::into_runtime);
        let engine_cfg = EngineConfig {
            model: thread.model.clone(),
            workspace: thread.workspace.clone(),
            allow_shell: thread.allow_shell,
            trust_mode: thread.trust_mode,
            notes_path: self.config.notes_path(),
            mcp_config_path: self.config.mcp_config_path(),
            skills_dir: self.config.skills_dir(),
            instructions: self.config.instructions_paths(),
            project_context_pack_enabled: self.config.project_context_pack_enabled(),
            translation_enabled: false,
            max_steps: 100,
            max_subagents: self.config.max_subagents().clamp(1, MAX_SUBAGENTS),
            features: self.config.features(),
            compaction,
            cycle: crate::cycle_manager::CycleConfig::default(),
            capacity: crate::core::capacity::CapacityControllerConfig::from_app_config(
                &self.config,
            ),
            todos: new_shared_todo_list(),
            plan_state: new_shared_plan_state(),
            max_spawn_depth: crate::tools::subagent::DEFAULT_MAX_SPAWN_DEPTH,
            network_policy,
            snapshots_enabled: self.config.snapshots_config().enabled,
            snapshots_max_workspace_bytes: self
                .config
                .snapshots_config()
                .max_workspace_gb
                .saturating_mul(1024 * 1024 * 1024),
            lsp_config,
            runtime_services: crate::tools::spec::RuntimeToolServices {
                task_manager: self.task_manager.lock().ok().and_then(|slot| slot.clone()),
                automations: self.automations.lock().ok().and_then(|slot| slot.clone()),
                task_data_dir: Some(self.manager_cfg.task_data_dir.clone()),
                active_task_id: thread.task_id.clone(),
                active_thread_id: Some(thread.id.clone()),
                shell_manager: None,
                hook_executor: None,
                handle_store: crate::tools::handle::new_shared_handle_store(),
                rlm_sessions: crate::rlm::session::new_shared_rlm_session_store(),
            },
            subagent_model_overrides: self.config.subagent_model_overrides(),
            memory_enabled: self.config.memory_enabled(),
            memory_path: self.config.memory_path(),
            vision_config: self.config.vision_model_config(),
            strict_tool_mode: self.config.strict_tool_mode.unwrap_or(false),
            goal_objective: None,
            locale_tag: crate::localization::resolve_locale(
                &crate::settings::Settings::load().unwrap_or_default().locale,
            )
            .tag()
            .to_string(),
            workshop: self.config.workshop.clone(),
            search_provider: self
                .config
                .search
                .as_ref()
                .and_then(|s| s.provider)
                .unwrap_or_default(),
            search_api_key: self.config.search.as_ref().and_then(|s| s.api_key.clone()),
        };

        let engine = spawn_engine(engine_cfg, &self.config);

        let turns = self.store.list_turns_for_thread(&thread.id)?;
        let session_messages = self.reconstruct_messages_from_turns(&turns)?;
        let sys_prompt = thread
            .system_prompt
            .as_ref()
            .map(|s| SystemPrompt::Text(s.clone()));
        if !session_messages.is_empty() || sys_prompt.is_some() {
            engine
                .send(Op::SyncSession {
                    session_id: None,
                    messages: session_messages,
                    system_prompt: sys_prompt,
                    model: thread.model.clone(),
                    workspace: thread.workspace.clone(),
                })
                .await
                .map_err(|e| anyhow!("Failed to sync thread session: {e}"))?;
        }

        let mut active = self.active.lock().await;
        let evicted = enforce_lru_capacity(&mut active, self.manager_cfg.max_active_threads);
        active.engines.insert(
            thread.id.clone(),
            ActiveThreadState {
                engine: engine.clone(),
                active_turn: None,
            },
        );
        touch_lru(&mut active.lru, &thread.id);
        drop(active);
        for handle in evicted {
            let _ = handle.send(Op::Shutdown).await;
        }
        Ok(engine)
    }

    fn reconstruct_messages_from_turns(&self, turns: &[TurnRecord]) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        for turn in turns {
            let items = self.store.list_items_for_turn(&turn.id)?;
            for item in items {
                match item.kind {
                    TurnItemKind::UserMessage => {
                        let text = item.detail.unwrap_or(item.summary);
                        messages.push(Message {
                            role: "user".to_string(),
                            content: vec![ContentBlock::Text {
                                text,
                                cache_control: None,
                            }],
                        });
                    }
                    TurnItemKind::AgentMessage => {
                        let text = item.detail.unwrap_or(item.summary);
                        messages.push(Message {
                            role: "assistant".to_string(),
                            content: vec![ContentBlock::Text {
                                text,
                                cache_control: None,
                            }],
                        });
                    }
                    _ => {}
                }
            }
        }
        Ok(messages)
    }

    async fn monitor_turn(
        &self,
        thread_id: String,
        turn_id: String,
        engine: EngineHandle,
    ) -> Result<()> {
        let mut current_message_item: Option<(String, String)> = None;
        let mut current_reasoning_item: Option<(String, String)> = None;
        let mut tool_items: HashMap<String, String> = HashMap::new();
        let mut compaction_items: HashMap<String, String> = HashMap::new();
        let mut turn_usage: Option<Usage> = None;
        let mut turn_status = RuntimeTurnStatus::Completed;
        let mut turn_error: Option<String> = None;

        loop {
            let event = {
                let mut rx = engine.rx_event.write().await;
                rx.recv().await
            };
            let Some(event) = event else {
                if self
                    .is_interrupt_requested(&thread_id, &turn_id)
                    .await
                    .unwrap_or(false)
                {
                    turn_status = RuntimeTurnStatus::Interrupted;
                }
                break;
            };

            match event {
                EngineEvent::TurnStarted { .. } => {
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "turn.lifecycle",
                        json!({ "status": "in_progress" }),
                    )
                    .await?;
                }
                EngineEvent::MessageStarted { .. } => {
                    let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: item_id.clone(),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::AgentMessage,
                        status: TurnItemLifecycleStatus::InProgress,
                        summary: String::new(),
                        detail: Some(String::new()),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: None,
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item_id),
                        "item.started",
                        json!({ "item": item }),
                    )
                    .await?;
                    current_message_item = Some((item_id, String::new()));
                }
                EngineEvent::MessageDelta { content, .. } => {
                    if let Some((item_id, text)) = current_message_item.as_mut() {
                        text.push_str(&content);
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(item_id),
                            "item.delta",
                            json!({ "delta": content, "kind": "agent_message" }),
                        )
                        .await?;
                    }
                }
                EngineEvent::MessageComplete { .. } => {
                    if let Some((item_id, text)) = current_message_item.take() {
                        let mut item = self.store.load_item(&item_id)?;
                        item.status = TurnItemLifecycleStatus::Completed;
                        item.summary = summarize_text(&text, SUMMARY_LIMIT);
                        item.detail = Some(text);
                        item.ended_at = Some(Utc::now());
                        self.store.save_item(&item)?;
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(&item_id),
                            "item.completed",
                            json!({ "item": item }),
                        )
                        .await?;
                    }
                }
                EngineEvent::ThinkingStarted { .. } => {
                    let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: item_id.clone(),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::AgentReasoning,
                        status: TurnItemLifecycleStatus::InProgress,
                        summary: String::new(),
                        detail: Some(String::new()),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: None,
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item_id),
                        "item.started",
                        json!({ "item": item }),
                    )
                    .await?;
                    current_reasoning_item = Some((item_id, String::new()));
                }
                EngineEvent::ThinkingDelta { content, .. } => {
                    if let Some((item_id, text)) = current_reasoning_item.as_mut() {
                        text.push_str(&content);
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(item_id),
                            "item.delta",
                            json!({ "delta": content, "kind": "agent_reasoning" }),
                        )
                        .await?;
                    }
                }
                EngineEvent::ThinkingComplete { .. } => {
                    if let Some((item_id, text)) = current_reasoning_item.take() {
                        let mut item = self.store.load_item(&item_id)?;
                        item.status = TurnItemLifecycleStatus::Completed;
                        item.summary = summarize_text(&text, SUMMARY_LIMIT);
                        item.detail = Some(text);
                        item.ended_at = Some(Utc::now());
                        self.store.save_item(&item)?;
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(&item_id),
                            "item.completed",
                            json!({ "item": item }),
                        )
                        .await?;
                    }
                }
                EngineEvent::ToolCallStarted { id, name, input } => {
                    let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                    tool_items.insert(id.clone(), item_id.clone());
                    let kind = tool_kind_for_name(&name);
                    let summary = summarize_text(&format!("{name} started"), SUMMARY_LIMIT);
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: item_id.clone(),
                        turn_id: turn_id.clone(),
                        kind,
                        status: TurnItemLifecycleStatus::InProgress,
                        summary,
                        detail: Some(serde_json::to_string(&input).unwrap_or_default()),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: None,
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item_id),
                        "item.started",
                        json!({ "item": item, "tool": { "id": id, "name": name, "input": input } }),
                    )
                    .await?;
                }
                EngineEvent::ToolCallProgress { id, output } => {
                    if let Some(item_id) = tool_items.get(&id) {
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(item_id),
                            "item.delta",
                            json!({ "delta": output, "kind": "tool_call" }),
                        )
                        .await?;
                    }
                }
                EngineEvent::ToolCallComplete { id, name, result } => {
                    if let Some(item_id) = tool_items.remove(&id) {
                        let mut item = self.store.load_item(&item_id)?;
                        let now = Utc::now();
                        item.ended_at = Some(now);
                        match result {
                            Ok(output) => {
                                item.status = if output.success {
                                    TurnItemLifecycleStatus::Completed
                                } else {
                                    TurnItemLifecycleStatus::Failed
                                };
                                item.summary = summarize_text(
                                    &format!("{name}: {}", output.content),
                                    SUMMARY_LIMIT,
                                );
                                item.detail = Some(output.content.clone());
                                item.metadata = output.metadata.clone();
                            }
                            Err(err) => {
                                item.status = TurnItemLifecycleStatus::Failed;
                                item.summary =
                                    summarize_text(&format!("{name} failed: {err}"), SUMMARY_LIMIT);
                                item.detail = Some(err.to_string());
                            }
                        }
                        self.store.save_item(&item)?;
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(&item_id),
                            if item.status == TurnItemLifecycleStatus::Completed {
                                "item.completed"
                            } else {
                                "item.failed"
                            },
                            json!({ "item": item }),
                        )
                        .await?;
                    }
                }
                EngineEvent::CompactionStarted { id, auto, message } => {
                    let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                    compaction_items.insert(id.clone(), item_id.clone());
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: item_id.clone(),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::ContextCompaction,
                        status: TurnItemLifecycleStatus::InProgress,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message.clone()),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: None,
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item_id),
                        "item.started",
                        json!({ "item": item, "auto": auto }),
                    )
                    .await?;
                }
                EngineEvent::CompactionCompleted {
                    id,
                    auto,
                    message,
                    messages_before,
                    messages_after,
                } => {
                    if let Some(item_id) = compaction_items.remove(&id) {
                        let mut item = self.store.load_item(&item_id)?;
                        item.status = TurnItemLifecycleStatus::Completed;
                        item.summary = summarize_text(&message, SUMMARY_LIMIT);
                        item.detail = Some(message);
                        item.ended_at = Some(Utc::now());
                        self.store.save_item(&item)?;
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(&item_id),
                            "item.completed",
                            json!({
                                "item": item,
                                "auto": auto,
                                "messages_before": messages_before,
                                "messages_after": messages_after,
                            }),
                        )
                        .await?;
                    }
                }
                EngineEvent::CompactionFailed { id, auto, message } => {
                    if let Some(item_id) = compaction_items.remove(&id) {
                        let mut item = self.store.load_item(&item_id)?;
                        item.status = TurnItemLifecycleStatus::Failed;
                        item.summary = summarize_text(&message, SUMMARY_LIMIT);
                        item.detail = Some(message);
                        item.ended_at = Some(Utc::now());
                        self.store.save_item(&item)?;
                        self.emit_event(
                            &thread_id,
                            Some(&turn_id),
                            Some(&item_id),
                            "item.failed",
                            json!({ "item": item, "auto": auto }),
                        )
                        .await?;
                    }
                }
                EngineEvent::CycleAdvanced { from, to, briefing } => {
                    // Surface the cycle boundary in the runtime event timeline so
                    // background-task subscribers and replay see it. The actual
                    // archive write is the engine's responsibility (see
                    // `cycle_manager::archive_cycle`); this event is informational.
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "cycle.advanced",
                        json!({
                            "from": from,
                            "to": to,
                            "briefing_tokens": briefing.token_estimate,
                            "cycle": briefing.cycle,
                            "timestamp": briefing.timestamp,
                        }),
                    )
                    .await?;
                }
                EngineEvent::CoherenceState {
                    state,
                    label,
                    description,
                    reason,
                } => {
                    let mut thread = self.store.load_thread(&thread_id)?;
                    thread.coherence_state = state;
                    thread.updated_at = Utc::now();
                    self.store.save_thread(&thread)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "coherence.state",
                        json!({
                            "state": state,
                            "label": label,
                            "description": description,
                            "reason": reason,
                            "thread": thread,
                        }),
                    )
                    .await?;
                }
                EngineEvent::CapacityDecision {
                    risk_band,
                    action,
                    reason,
                    ..
                } => {
                    let message = format!(
                        "Capacity decision: risk={risk_band} action={action} reason={reason}"
                    );
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "item.completed",
                        json!({ "item": item }),
                    )
                    .await?;
                }
                EngineEvent::CapacityIntervention {
                    action,
                    before_prompt_tokens,
                    after_prompt_tokens,
                    replay_outcome,
                    replan_performed,
                    ..
                } => {
                    let message = format!(
                        "Capacity intervention: {action} (~{before_prompt_tokens} -> ~{after_prompt_tokens}) replay={:?} replan={replan_performed}",
                        replay_outcome
                    );
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "item.completed",
                        json!({ "item": item }),
                    )
                    .await?;
                }
                EngineEvent::CapacityMemoryPersistFailed { action, error, .. } => {
                    let message =
                        format!("Capacity memory persist failed: action={action} error={error}");
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Failed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "item.failed",
                        json!({ "item": item }),
                    )
                    .await?;
                }
                EngineEvent::AgentSpawned { id, prompt } => {
                    let message = format!(
                        "Sub-agent {id} spawned: {}",
                        summarize_text(&prompt, SUMMARY_LIMIT)
                    );
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "agent.spawned",
                        json!({ "item": item, "agent_id": id }),
                    )
                    .await?;
                }
                EngineEvent::AgentProgress { id, status } => {
                    let message = format!("Sub-agent {id}: {status}");
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "agent.progress",
                        json!({ "item": item, "agent_id": id }),
                    )
                    .await?;
                }
                EngineEvent::AgentComplete { id, result } => {
                    let message = format!(
                        "Sub-agent {id} completed: {}",
                        summarize_text(&result, SUMMARY_LIMIT)
                    );
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "agent.completed",
                        json!({ "item": item, "agent_id": id }),
                    )
                    .await?;
                }
                EngineEvent::AgentList { agents } => {
                    let running = agents
                        .iter()
                        .filter(|agent| matches!(agent.status, SubAgentStatus::Running))
                        .count();
                    let interrupted = agents
                        .iter()
                        .filter(|agent| matches!(agent.status, SubAgentStatus::Interrupted(_)))
                        .count();
                    let completed = agents
                        .iter()
                        .filter(|agent| matches!(agent.status, SubAgentStatus::Completed))
                        .count();
                    let message = format!(
                        "Sub-agent list refreshed: {} total ({running} running, {interrupted} interrupted, {completed} completed)",
                        agents.len()
                    );
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "agent.list",
                        json!({ "item": item, "agents": agents }),
                    )
                    .await?;
                }
                EngineEvent::ApprovalRequired {
                    id,
                    tool_name,
                    description,
                    ..
                } => {
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "approval.required",
                        json!({
                            "id": id,
                            "approval_id": id,
                            "tool_name": tool_name,
                            "description": description,
                        }),
                    )
                    .await?;

                    let Some((auto_approve, trust_mode)) =
                        self.active_turn_flags(&thread_id, &turn_id).await
                    else {
                        let _ = engine.deny_tool_call(id).await;
                        continue;
                    };

                    if auto_approve || trust_mode {
                        match Self::approval_decision(auto_approve, trust_mode, false) {
                            RuntimeApprovalDecision::ApproveTool => {
                                let _ = engine.approve_tool_call(id).await;
                            }
                            RuntimeApprovalDecision::DenyTool
                            | RuntimeApprovalDecision::RetryWithFullAccess => {
                                let _ = engine.deny_tool_call(id).await;
                            }
                        }
                        continue;
                    }

                    let rx = self.register_pending_approval(&id);
                    match tokio::time::timeout(APPROVAL_DECISION_TIMEOUT, rx).await {
                        Ok(Ok(ExternalApprovalDecision::Allow { remember })) => {
                            if remember {
                                self.remember_thread_auto_approve(&thread_id).await;
                            }
                            self.emit_event(
                                &thread_id,
                                Some(&turn_id),
                                None,
                                "approval.decided",
                                json!({
                                    "approval_id": id,
                                    "decision": "allow",
                                    "remember": remember,
                                }),
                            )
                            .await
                            .ok();
                            let _ = engine.approve_tool_call(id).await;
                        }
                        Ok(Ok(ExternalApprovalDecision::Deny { remember })) => {
                            self.emit_event(
                                &thread_id,
                                Some(&turn_id),
                                None,
                                "approval.decided",
                                json!({
                                    "approval_id": id,
                                    "decision": "deny",
                                    "remember": remember,
                                }),
                            )
                            .await
                            .ok();
                            let _ = engine.deny_tool_call(id).await;
                        }
                        Ok(Err(_recv_err)) => {
                            self.cancel_pending_approval(&id);
                            let _ = engine.deny_tool_call(id).await;
                        }
                        Err(_timeout) => {
                            self.cancel_pending_approval(&id);
                            self.emit_event(
                                &thread_id,
                                Some(&turn_id),
                                None,
                                "approval.timeout",
                                json!({
                                    "approval_id": id,
                                    "timeout_secs": APPROVAL_DECISION_TIMEOUT.as_secs(),
                                }),
                            )
                            .await
                            .ok();
                            let _ = engine.deny_tool_call(id).await;
                        }
                    }
                }
                EngineEvent::ElevationRequired {
                    tool_id,
                    tool_name,
                    denial_reason,
                    ..
                } => {
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "sandbox.denied",
                        json!({
                            "tool_id": tool_id,
                            "tool_name": tool_name,
                            "reason": denial_reason,
                        }),
                    )
                    .await?;
                    let (auto_approve, trust_mode) = self
                        .active_turn_flags(&thread_id, &turn_id)
                        .await
                        .unwrap_or((false, false));
                    match Self::approval_decision(auto_approve, trust_mode, true) {
                        RuntimeApprovalDecision::RetryWithFullAccess => {
                            let _ = engine
                                .retry_tool_with_policy(
                                    tool_id,
                                    crate::sandbox::SandboxPolicy::DangerFullAccess,
                                )
                                .await;
                        }
                        RuntimeApprovalDecision::ApproveTool
                        | RuntimeApprovalDecision::DenyTool => {
                            let _ = engine.deny_tool_call(tool_id).await;
                        }
                    }
                }
                EngineEvent::Status { message } => {
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Status,
                        status: TurnItemLifecycleStatus::Completed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message.clone()),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "item.completed",
                        json!({ "item": item }),
                    )
                    .await?;
                }
                EngineEvent::Error { envelope, .. } => {
                    turn_status = RuntimeTurnStatus::Failed;
                    turn_error = Some(envelope.message.clone());
                    let message = envelope.message.clone();
                    let item = TurnItemRecord {
                        schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                        id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                        turn_id: turn_id.clone(),
                        kind: TurnItemKind::Error,
                        status: TurnItemLifecycleStatus::Failed,
                        summary: summarize_text(&message, SUMMARY_LIMIT),
                        detail: Some(message),
                        metadata: None,
                        artifact_refs: Vec::new(),
                        started_at: Some(Utc::now()),
                        ended_at: Some(Utc::now()),
                    };
                    self.store.save_item(&item)?;
                    self.attach_item_to_turn(&turn_id, &item.id)?;
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        Some(&item.id),
                        "item.failed",
                        json!({ "item": item }),
                    )
                    .await?;
                }
                EngineEvent::TurnComplete {
                    usage,
                    status,
                    error,
                } => {
                    turn_usage = Some(usage);
                    turn_status = match status {
                        TurnOutcomeStatus::Completed => RuntimeTurnStatus::Completed,
                        TurnOutcomeStatus::Interrupted => RuntimeTurnStatus::Interrupted,
                        TurnOutcomeStatus::Failed => RuntimeTurnStatus::Failed,
                    };
                    if let Some(err) = error {
                        turn_error = Some(err);
                    }
                    break;
                }
                _ => {}
            }
        }

        if self
            .is_interrupt_requested(&thread_id, &turn_id)
            .await
            .unwrap_or(false)
        {
            turn_status = RuntimeTurnStatus::Interrupted;
        }

        if let Some((item_id, text)) = current_message_item.take() {
            let mut item = self.store.load_item(&item_id)?;
            if turn_status == RuntimeTurnStatus::Interrupted {
                item.status = TurnItemLifecycleStatus::Interrupted;
            } else {
                item.status = TurnItemLifecycleStatus::Completed;
            }
            item.summary = summarize_text(&text, SUMMARY_LIMIT);
            item.detail = Some(text);
            item.ended_at = Some(Utc::now());
            self.store.save_item(&item)?;
            self.emit_event(
                &thread_id,
                Some(&turn_id),
                Some(&item_id),
                if item.status == TurnItemLifecycleStatus::Interrupted {
                    "item.interrupted"
                } else {
                    "item.completed"
                },
                json!({ "item": item }),
            )
            .await?;
        }

        if let Some((item_id, text)) = current_reasoning_item.take() {
            let mut item = self.store.load_item(&item_id)?;
            if turn_status == RuntimeTurnStatus::Interrupted {
                item.status = TurnItemLifecycleStatus::Interrupted;
            } else {
                item.status = TurnItemLifecycleStatus::Completed;
            }
            item.summary = summarize_text(&text, SUMMARY_LIMIT);
            item.detail = Some(text);
            item.ended_at = Some(Utc::now());
            self.store.save_item(&item)?;
            self.emit_event(
                &thread_id,
                Some(&turn_id),
                Some(&item_id),
                if item.status == TurnItemLifecycleStatus::Interrupted {
                    "item.interrupted"
                } else {
                    "item.completed"
                },
                json!({ "item": item }),
            )
            .await?;
        }

        let ended_at = Utc::now();
        let mut turn = self.store.load_turn(&turn_id)?;
        turn.status = turn_status;
        turn.ended_at = Some(ended_at);
        turn.duration_ms = turn.started_at.map(|start| duration_ms(start, ended_at));
        turn.usage = turn_usage;
        turn.error = turn_error;
        self.store.save_turn(&turn)?;

        let mut thread = self.get_thread(&thread_id).await?;
        thread.latest_turn_id = Some(turn_id.clone());
        thread.updated_at = Utc::now();
        self.store.save_thread(&thread)?;

        self.emit_event(
            &thread_id,
            Some(&turn_id),
            None,
            "turn.completed",
            json!({ "turn": turn.clone() }),
        )
        .await?;

        {
            let mut active = self.active.lock().await;
            if let Some(state) = active.engines.get_mut(&thread_id)
                && state
                    .active_turn
                    .as_ref()
                    .is_some_and(|t| t.turn_id == turn_id)
            {
                state.active_turn = None;
            }
            touch_lru(&mut active.lru, &thread_id);
        }

        Ok(())
    }

    fn attach_item_to_turn(&self, turn_id: &str, item_id: &str) -> Result<()> {
        let mut turn = self.store.load_turn(turn_id)?;
        if !turn.item_ids.iter().any(|id| id == item_id) {
            turn.item_ids.push(item_id.to_string());
            self.store.save_turn(&turn)?;
        }
        Ok(())
    }

    async fn is_interrupt_requested(&self, thread_id: &str, turn_id: &str) -> Result<bool> {
        let active = self.active.lock().await;
        let Some(state) = active.engines.get(thread_id) else {
            return Ok(false);
        };
        let Some(turn) = state.active_turn.as_ref() else {
            return Ok(false);
        };
        Ok(turn.turn_id == turn_id && turn.interrupt_requested)
    }

    async fn active_turn_flags(&self, thread_id: &str, turn_id: &str) -> Option<(bool, bool)> {
        let active = self.active.lock().await;
        let state = active.engines.get(thread_id)?;
        let turn = state.active_turn.as_ref()?;
        if turn.turn_id != turn_id {
            return None;
        }
        Some((turn.auto_approve, turn.trust_mode))
    }

    fn approval_decision(
        auto_approve: bool,
        trust_mode: bool,
        requires_full_access: bool,
    ) -> RuntimeApprovalDecision {
        if !auto_approve {
            return RuntimeApprovalDecision::DenyTool;
        }
        if requires_full_access {
            if trust_mode {
                RuntimeApprovalDecision::RetryWithFullAccess
            } else {
                RuntimeApprovalDecision::DenyTool
            }
        } else {
            RuntimeApprovalDecision::ApproveTool
        }
    }

    fn recover_interrupted_state(&self) -> Result<()> {
        let now = Utc::now();
        for mut thread in self.store.list_threads()? {
            let mut thread_changed = false;
            for mut turn in self.store.list_turns_for_thread(&thread.id)? {
                if !matches!(
                    turn.status,
                    RuntimeTurnStatus::Queued | RuntimeTurnStatus::InProgress
                ) {
                    continue;
                }

                turn.status = RuntimeTurnStatus::Interrupted;
                turn.error = Some(RUNTIME_RESTART_REASON.to_string());
                turn.ended_at = Some(now);
                if let Some(started_at) = turn.started_at {
                    let elapsed = now.signed_duration_since(started_at);
                    turn.duration_ms = Some(elapsed.num_milliseconds().max(0) as u64);
                }
                self.store.save_turn(&turn)?;

                for item_id in &turn.item_ids {
                    let mut item = self.store.load_item(item_id)?;
                    if matches!(
                        item.status,
                        TurnItemLifecycleStatus::Queued | TurnItemLifecycleStatus::InProgress
                    ) {
                        item.status = TurnItemLifecycleStatus::Interrupted;
                        item.ended_at = Some(now);
                        self.store.save_item(&item)?;
                    }
                }

                thread.updated_at = now;
                thread_changed = true;
            }

            if thread_changed {
                self.store.save_thread(&thread)?;
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn install_test_engine(
        &self,
        thread_id: &str,
        engine: EngineHandle,
    ) -> Result<()> {
        let _ = self.get_thread(thread_id).await?;
        let mut active = self.active.lock().await;
        active.engines.insert(
            thread_id.to_string(),
            ActiveThreadState {
                engine,
                active_turn: None,
            },
        );
        touch_lru(&mut active.lru, thread_id);
        Ok(())
    }
}

fn touch_lru(lru: &mut VecDeque<String>, thread_id: &str) {
    if let Some(idx) = lru.iter().position(|id| id == thread_id) {
        lru.remove(idx);
    }
    lru.push_back(thread_id.to_string());
}

fn enforce_lru_capacity(
    active: &mut ActiveThreads,
    max_active_threads: usize,
) -> Vec<EngineHandle> {
    let mut evicted = Vec::new();
    if max_active_threads == 0 || active.engines.len() < max_active_threads {
        return evicted;
    }
    let protected = active
        .engines
        .iter()
        .filter_map(|(thread_id, state)| {
            if state.active_turn.is_some() {
                Some(thread_id.clone())
            } else {
                None
            }
        })
        .collect::<HashSet<_>>();

    let scan_limit = active.lru.len();
    for _ in 0..scan_limit {
        let Some(candidate) = active.lru.pop_front() else {
            break;
        };
        if protected.contains(&candidate) {
            active.lru.push_back(candidate);
            continue;
        }
        if let Some(state) = active.engines.remove(&candidate) {
            evicted.push(state.engine);
        }
        break;
    }
    evicted
}

fn parse_mode(mode: &str) -> AppMode {
    match mode.trim().to_ascii_lowercase().as_str() {
        "plan" => AppMode::Plan,
        "yolo" => AppMode::Yolo,
        _ => AppMode::Agent,
    }
}

fn tool_kind_for_name(name: &str) -> TurnItemKind {
    let lower = name.to_ascii_lowercase();
    if lower == "exec_shell" || lower == "exec_shell_wait" || lower == "exec_shell_interact" {
        return TurnItemKind::CommandExecution;
    }
    if lower.contains("patch") || lower.contains("write") || lower.contains("edit") {
        return TurnItemKind::FileChange;
    }
    TurnItemKind::ToolCall
}

/// One sub-agent rebind hint extracted from a thread's persisted event
/// timeline (issue #128). When the TUI resumes a session that was
/// mid-fanout, the in-transcript card stack is empty — these hints let the
/// UI know which agent_ids were live (or recently terminal) so it can
/// reconstruct the matching `DelegateCard` / `FanoutCard` placeholders
/// before fresh mailbox envelopes arrive on a re-attached engine.
///
/// The helper is the testable contract here — actual TUI wire-up to the
/// resume flow is a follow-up; the runtime API consumer (`runtime_api.rs`)
/// can already call `resume_thread_with_agent_rebind` to drive it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // consumed by #128 follow-up TUI resume wiring; tested here.
pub struct AgentRebindHint {
    pub agent_id: String,
    pub status: AgentRebindStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AgentRebindStatus {
    Spawned,
    InProgress,
    Completed,
}

/// Collapse a chronologically ordered slice of `RuntimeEventRecord` into
/// the latest known status per `agent_id`. Drops entries that aren't in
/// the `agent.*` family. Cards built from these hints are immediately
/// open to mutation by subsequent live mailbox envelopes (each envelope's
/// `agent_id` matches one already in the rebind map).
#[must_use]
#[allow(dead_code)]
pub fn collect_agent_rebind_hints(events: &[RuntimeEventRecord]) -> Vec<AgentRebindHint> {
    use std::collections::BTreeMap;
    let mut latest: BTreeMap<String, AgentRebindStatus> = BTreeMap::new();
    for event in events {
        let id = match event.payload.get("agent_id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let next_status = match event.event.as_str() {
            "agent.spawned" => Some(AgentRebindStatus::Spawned),
            "agent.progress" => Some(AgentRebindStatus::InProgress),
            "agent.completed" => Some(AgentRebindStatus::Completed),
            _ => None,
        };
        if let Some(status) = next_status {
            // Don't downgrade Completed → InProgress on out-of-order events.
            let entry = latest.entry(id).or_insert(status);
            if !matches!(*entry, AgentRebindStatus::Completed) {
                *entry = status;
            }
        }
    }
    latest
        .into_iter()
        .map(|(agent_id, status)| AgentRebindHint { agent_id, status })
        .collect()
}

pub fn summarize_text(text: &str, limit: usize) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::engine::{MockApprovalEvent, mock_engine_handle};
    use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
    use std::time::{Duration, Instant};
    use tokio::sync::oneshot;
    use tokio::time::sleep;
    use uuid::Uuid;

    fn test_runtime_dir() -> PathBuf {
        std::env::temp_dir().join(format!("deepseek-runtime-threads-{}", Uuid::new_v4()))
    }

    fn test_manager_config(data_dir: PathBuf) -> RuntimeThreadManagerConfig {
        RuntimeThreadManagerConfig {
            task_data_dir: data_dir.clone(),
            data_dir,
            max_active_threads: 4,
        }
    }

    fn test_manager(data_dir: PathBuf) -> Result<RuntimeThreadManager> {
        RuntimeThreadManager::open(
            Config::default(),
            PathBuf::from("."),
            test_manager_config(data_dir),
        )
    }

    fn sample_thread(thread_id: &str) -> ThreadRecord {
        let now = Utc::now();
        ThreadRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: thread_id.to_string(),
            created_at: now,
            updated_at: now,
            model: DEFAULT_TEXT_MODEL.to_string(),
            workspace: PathBuf::from("."),
            mode: AppMode::Agent.as_setting().to_string(),
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            latest_turn_id: None,
            latest_response_bookmark: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            title: None,
            coherence_state: CoherenceState::default(),
        }
    }

    fn sample_turn(thread_id: &str, turn_id: &str, status: RuntimeTurnStatus) -> TurnRecord {
        let now = Utc::now();
        TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: turn_id.to_string(),
            thread_id: thread_id.to_string(),
            status,
            input_summary: "sample".to_string(),
            created_at: now,
            started_at: Some(now),
            ended_at: None,
            duration_ms: None,
            usage: None,
            error: None,
            item_ids: Vec::new(),
            steer_count: 0,
        }
    }

    fn sample_item(
        turn_id: &str,
        item_id: &str,
        status: TurnItemLifecycleStatus,
    ) -> TurnItemRecord {
        TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: item_id.to_string(),
            turn_id: turn_id.to_string(),
            kind: TurnItemKind::Status,
            status,
            summary: "sample item".to_string(),
            detail: None,
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(Utc::now()),
            ended_at: None,
        }
    }

    async fn install_mock_engine(
        manager: &RuntimeThreadManager,
        thread_id: &str,
    ) -> crate::core::engine::MockEngineHandle {
        let harness = mock_engine_handle();
        let mut active = manager.active.lock().await;
        active.engines.insert(
            thread_id.to_string(),
            ActiveThreadState {
                engine: harness.handle.clone(),
                active_turn: None,
            },
        );
        touch_lru(&mut active.lru, thread_id);
        harness
    }

    async fn wait_for_terminal_turn(
        manager: &RuntimeThreadManager,
        turn_id: &str,
        timeout: Duration,
    ) -> Result<TurnRecord> {
        let deadline = Instant::now() + timeout;
        loop {
            let turn = manager.store.load_turn(turn_id)?;
            if matches!(
                turn.status,
                RuntimeTurnStatus::Completed
                    | RuntimeTurnStatus::Failed
                    | RuntimeTurnStatus::Interrupted
                    | RuntimeTurnStatus::Canceled
            ) {
                return Ok(turn);
            }
            if Instant::now() >= deadline {
                bail!("Timed out waiting for turn {turn_id}");
            }
            sleep(Duration::from_millis(20)).await;
        }
    }

    #[test]
    fn store_load_thread_rejects_newer_schema_version() {
        let dir = test_runtime_dir();
        let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

        // Construct a thread record persisted with a future schema version.
        let mut thread = sample_thread("thr_future");
        thread.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

        // Bypass save_thread (which would respect our local schema_version)
        // by writing the JSON directly so we can simulate a future writer.
        let path = store.threads_dir.join(format!("{}.json", thread.id));
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
        let payload = serde_json::to_string(&thread).expect("serialize thread");
        std::fs::write(&path, payload).expect("write thread");

        let err = store
            .load_thread(&thread.id)
            .expect_err("load_thread must reject newer schema");
        let msg = format!("{err:#}");
        assert!(msg.contains("newer than supported"), "got: {msg}");

        // Cleanup so we don't leak across tests.
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn current_runtime_schema_version_is_two_on_v066() {
        // Locks the bump in (issue #124). Bump deliberately when persisted
        // shape changes.
        assert_eq!(CURRENT_RUNTIME_SCHEMA_VERSION, 2);
    }

    #[test]
    fn store_rejects_path_like_record_ids() {
        let dir = test_runtime_dir();
        let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

        let err = store
            .load_thread("../outside")
            .expect_err("path traversal id should fail");
        assert!(
            format!("{err:#}").contains("unsupported characters"),
            "got: {err:#}"
        );

        let mut thread = sample_thread("thr_bad/id");
        let err = store
            .save_thread(&thread)
            .expect_err("path separator id should fail");
        assert!(
            format!("{err:#}").contains("unsupported characters"),
            "got: {err:#}"
        );

        thread.id = " thr_bad".to_string();
        let err = store
            .save_thread(&thread)
            .expect_err("whitespace id should fail");
        assert!(format!("{err:#}").contains("whitespace"), "got: {err:#}");

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_load_turn_rejects_newer_schema_version() {
        let dir = test_runtime_dir();
        let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

        let mut turn = sample_turn("thr_t", "trn_future", RuntimeTurnStatus::InProgress);
        turn.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

        let path = store.turns_dir.join(format!("{}.json", turn.id));
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
        std::fs::write(&path, serde_json::to_string(&turn).expect("serialize turn"))
            .expect("write turn");

        let err = store
            .load_turn(&turn.id)
            .expect_err("load_turn must reject newer schema");
        assert!(
            format!("{err:#}").contains("newer than supported"),
            "got: {err:#}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn store_load_item_rejects_newer_schema_version() {
        let dir = test_runtime_dir();
        let store = RuntimeThreadStore::open(dir.clone()).expect("open store");

        let mut item = sample_item("trn_t", "itm_future", TurnItemLifecycleStatus::InProgress);
        item.schema_version = CURRENT_RUNTIME_SCHEMA_VERSION + 1;

        let path = store.items_dir.join(format!("{}.json", item.id));
        std::fs::create_dir_all(path.parent().unwrap()).expect("mkdirs");
        std::fs::write(&path, serde_json::to_string(&item).expect("serialize item"))
            .expect("write item");

        let err = store
            .load_item(&item.id)
            .expect_err("load_item must reject newer schema");
        assert!(
            format!("{err:#}").contains("newer than supported"),
            "got: {err:#}"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn enforce_lru_capacity_does_not_loop_when_all_threads_are_active() {
        let mut active = ActiveThreads::default();
        let harness_a = mock_engine_handle();
        let harness_b = mock_engine_handle();

        active.engines.insert(
            "thr_a".to_string(),
            ActiveThreadState {
                engine: harness_a.handle,
                active_turn: Some(ActiveTurnState {
                    turn_id: "turn_a".to_string(),
                    interrupt_requested: false,
                    auto_approve: true,
                    trust_mode: false,
                }),
            },
        );
        active.engines.insert(
            "thr_b".to_string(),
            ActiveThreadState {
                engine: harness_b.handle,
                active_turn: Some(ActiveTurnState {
                    turn_id: "turn_b".to_string(),
                    interrupt_requested: false,
                    auto_approve: true,
                    trust_mode: false,
                }),
            },
        );
        active.lru.push_back("thr_a".to_string());
        active.lru.push_back("thr_b".to_string());

        let evicted = enforce_lru_capacity(&mut active, 2);
        assert!(evicted.is_empty(), "no idle threads should be evicted");
        assert_eq!(active.engines.len(), 2);
        assert_eq!(active.lru.len(), 2);
    }

    #[test]
    fn approval_decision_matches_auto_approve_and_trust_mode() {
        assert!(matches!(
            RuntimeThreadManager::approval_decision(false, false, false),
            RuntimeApprovalDecision::DenyTool
        ));
        assert!(matches!(
            RuntimeThreadManager::approval_decision(true, false, false),
            RuntimeApprovalDecision::ApproveTool
        ));
        assert!(matches!(
            RuntimeThreadManager::approval_decision(true, false, true),
            RuntimeApprovalDecision::DenyTool
        ));
        assert!(matches!(
            RuntimeThreadManager::approval_decision(true, true, true),
            RuntimeApprovalDecision::RetryWithFullAccess
        ));
    }

    #[test]
    fn open_recovers_queued_and_in_progress_turns() -> Result<()> {
        let runtime_dir = test_runtime_dir();
        let store = RuntimeThreadStore::open(runtime_dir.clone())?;
        let thread = sample_thread("thr_recover");
        store.save_thread(&thread)?;

        let mut queued_turn = sample_turn(&thread.id, "turn_queued", RuntimeTurnStatus::Queued);
        let mut in_progress_turn =
            sample_turn(&thread.id, "turn_running", RuntimeTurnStatus::InProgress);
        let completed_turn = sample_turn(&thread.id, "turn_done", RuntimeTurnStatus::Completed);

        let queued_item = sample_item(
            &queued_turn.id,
            "item_queued",
            TurnItemLifecycleStatus::Queued,
        );
        let in_progress_item = sample_item(
            &in_progress_turn.id,
            "item_running",
            TurnItemLifecycleStatus::InProgress,
        );
        let completed_item = sample_item(
            &completed_turn.id,
            "item_done",
            TurnItemLifecycleStatus::Completed,
        );

        queued_turn.item_ids = vec![queued_item.id.clone()];
        in_progress_turn.item_ids = vec![in_progress_item.id.clone()];

        store.save_item(&queued_item)?;
        store.save_item(&in_progress_item)?;
        store.save_item(&completed_item)?;
        store.save_turn(&queued_turn)?;
        store.save_turn(&in_progress_turn)?;
        store.save_turn(&completed_turn)?;

        let manager = test_manager(runtime_dir)?;

        let queued_turn = manager.store.load_turn(&queued_turn.id)?;
        assert_eq!(queued_turn.status, RuntimeTurnStatus::Interrupted);
        assert_eq!(queued_turn.error.as_deref(), Some(RUNTIME_RESTART_REASON));
        assert!(queued_turn.ended_at.is_some());
        assert!(queued_turn.duration_ms.is_some());

        let in_progress_turn = manager.store.load_turn(&in_progress_turn.id)?;
        assert_eq!(in_progress_turn.status, RuntimeTurnStatus::Interrupted);
        assert_eq!(
            in_progress_turn.error.as_deref(),
            Some(RUNTIME_RESTART_REASON)
        );
        assert!(in_progress_turn.ended_at.is_some());
        assert!(in_progress_turn.duration_ms.is_some());

        let completed_turn = manager.store.load_turn(&completed_turn.id)?;
        assert_eq!(completed_turn.status, RuntimeTurnStatus::Completed);
        assert!(completed_turn.error.is_none());

        let queued_item = manager.store.load_item("item_queued")?;
        assert_eq!(queued_item.status, TurnItemLifecycleStatus::Interrupted);
        assert!(queued_item.ended_at.is_some());

        let in_progress_item = manager.store.load_item("item_running")?;
        assert_eq!(
            in_progress_item.status,
            TurnItemLifecycleStatus::Interrupted
        );
        assert!(in_progress_item.ended_at.is_some());

        let completed_item = manager.store.load_item("item_done")?;
        assert_eq!(completed_item.status, TurnItemLifecycleStatus::Completed);

        Ok(())
    }

    #[tokio::test]
    async fn thread_lifecycle_persists_across_restart() -> Result<()> {
        let runtime_dir = test_runtime_dir();
        let manager = test_manager(runtime_dir.clone())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        tokio::spawn(async move {
            if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: "engine_turn_1".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageStarted { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageDelta {
                        index: 0,
                        content: "mock response".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageComplete { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::CoherenceState {
                        state: CoherenceState::GettingCrowded,
                        label: "getting crowded".to_string(),
                        description: "The session is approaching context pressure.".to_string(),
                        reason: "test capacity signal".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                    })
                    .await;
            }
        });

        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "first prompt".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        let completed = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(completed.status, RuntimeTurnStatus::Completed);

        drop(manager);

        let reopened = test_manager(runtime_dir)?;
        let detail = reopened.get_thread_detail(&thread.id).await?;
        assert_eq!(detail.thread.id, thread.id);
        assert_eq!(
            detail.thread.coherence_state,
            CoherenceState::GettingCrowded
        );
        assert_eq!(detail.turns.len(), 1);
        assert!(detail.latest_seq >= 1);
        assert!(!detail.items.is_empty());
        let events = reopened.events_since(&thread.id, None)?;
        assert!(
            events.iter().any(|ev| ev.event == "turn.completed"),
            "expected turn.completed event after restart"
        );
        assert!(
            events.iter().any(|ev| ev.event == "coherence.state"
                && ev.payload.get("state").and_then(serde_json::Value::as_str)
                    == Some("getting_crowded")),
            "expected machine-readable coherence event after restart"
        );
        Ok(())
    }

    #[tokio::test]
    async fn create_thread_defaults_auto_approve_to_false() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        assert!(!thread.auto_approve);
        assert_eq!(thread.coherence_state, CoherenceState::Healthy);
        Ok(())
    }

    #[tokio::test]
    async fn start_turn_passes_effective_auto_approve_to_engine() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(false),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;

        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "override approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: Some(true),
                },
            )
            .await?;

        match rx_op.recv().await {
            Some(Op::SendMessage { auto_approve, .. }) => assert!(auto_approve),
            other => panic!("expected SendMessage op, got {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn start_turn_can_override_thread_auto_approve_to_false() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;

        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "disable approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: Some(false),
                },
            )
            .await?;

        match rx_op.recv().await {
            Some(Op::SendMessage { auto_approve, .. }) => assert!(!auto_approve),
            other => panic!("expected SendMessage op, got {other:?}"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn compact_thread_preserves_thread_auto_approve_policy() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(false),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;

        let turn = manager
            .compact_thread(&thread.id, CompactThreadRequest::default())
            .await?;

        assert!(matches!(rx_op.recv().await, Some(Op::CompactContext)));
        assert_eq!(
            manager.active_turn_flags(&thread.id, &turn.id).await,
            Some((false, false))
        );

        Ok(())
    }

    #[tokio::test]
    async fn compact_thread_with_real_engine_reaches_terminal_status() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let turn = manager
            .compact_thread(&thread.id, CompactThreadRequest::default())
            .await?;
        let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;

        assert!(matches!(
            terminal.status,
            RuntimeTurnStatus::Completed | RuntimeTurnStatus::Failed
        ));
        assert!(
            terminal.ended_at.is_some(),
            "manual compaction should reach a terminal turn state"
        );
        assert_eq!(manager.active_turn_flags(&thread.id, &turn.id).await, None);

        let expected_status = match terminal.status {
            RuntimeTurnStatus::Completed => "completed",
            RuntimeTurnStatus::Failed => "failed",
            other => panic!("unexpected non-terminal compaction status: {other:?}"),
        };
        let events = manager.events_since(&thread.id, None)?;
        assert!(events.iter().any(|ev| {
            ev.event == "turn.completed"
                && ev
                    .payload
                    .get("turn")
                    .and_then(|turn| turn.get("status"))
                    .and_then(Value::as_str)
                    == Some(expected_status)
        }));
        Ok(())
    }

    #[tokio::test]
    async fn multi_turn_continuity_same_thread() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        tokio::spawn(async move {
            let mut turn_index = 0u8;
            while let Some(op) = rx_op.recv().await {
                if !matches!(op, Op::SendMessage { .. }) {
                    continue;
                }
                turn_index = turn_index.saturating_add(1);
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: format!("engine_turn_{turn_index}"),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageStarted { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageDelta {
                        index: 0,
                        content: format!("reply {turn_index}"),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageComplete { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 5,
                            output_tokens: 5,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                    })
                    .await;
                if turn_index >= 2 {
                    break;
                }
            }
        });

        let turn_1 = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "first".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        let turn_1 = wait_for_terminal_turn(&manager, &turn_1.id, Duration::from_secs(2)).await?;
        assert_eq!(turn_1.status, RuntimeTurnStatus::Completed);

        let turn_2 = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "second".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        let turn_2 = wait_for_terminal_turn(&manager, &turn_2.id, Duration::from_secs(2)).await?;
        assert_eq!(turn_2.status, RuntimeTurnStatus::Completed);

        let detail = manager.get_thread_detail(&thread.id).await?;
        assert_eq!(
            detail.thread.latest_turn_id.as_deref(),
            Some(turn_2.id.as_str())
        );
        assert_eq!(detail.turns.len(), 2);
        assert!(detail.items.iter().any(|item| {
            item.kind == TurnItemKind::UserMessage && item.detail.as_deref() == Some("first")
        }));
        assert!(detail.items.iter().any(|item| {
            item.kind == TurnItemKind::UserMessage && item.detail.as_deref() == Some("second")
        }));

        let events = manager.events_since(&thread.id, None)?;
        let started = events
            .iter()
            .filter(|ev| ev.event == "turn.started")
            .count();
        let completed = events
            .iter()
            .filter(|ev| ev.event == "turn.completed")
            .count();
        assert_eq!(started, 2);
        assert_eq!(completed, 2);
        Ok(())
    }

    #[tokio::test]
    async fn interrupt_turn_marks_interrupted_after_cleanup() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        let cancel_token = harness.cancel_token;
        let cleanup_delay = Duration::from_millis(140);
        tokio::spawn(async move {
            if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: "engine_turn_interrupt".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageStarted { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageDelta {
                        index: 0,
                        content: "partial".to_string(),
                    })
                    .await;
                cancel_token.cancelled().await;
                sleep(cleanup_delay).await;
            }
        });

        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "interrupt me".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;

        sleep(Duration::from_millis(20)).await;
        let interrupted_at = Instant::now();
        let interrupt_result = manager.interrupt_turn(&thread.id, &turn.id).await?;
        assert_eq!(interrupt_result.status, RuntimeTurnStatus::InProgress);

        let final_turn = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(3)).await?;
        assert_eq!(final_turn.status, RuntimeTurnStatus::Interrupted);
        assert!(
            interrupted_at.elapsed() >= cleanup_delay,
            "turn transitioned before cleanup finished"
        );

        let events = manager.events_since(&thread.id, None)?;
        let interrupt_seq = events
            .iter()
            .find(|ev| ev.event == "turn.interrupt_requested")
            .map(|ev| ev.seq)
            .context("missing turn.interrupt_requested event")?;
        let completed = events
            .iter()
            .find(|ev| ev.event == "turn.completed")
            .context("missing turn.completed event")?;
        assert!(completed.seq > interrupt_seq);
        assert_eq!(
            completed
                .payload
                .get("turn")
                .and_then(|turn| turn.get("status"))
                .and_then(Value::as_str),
            Some("interrupted")
        );
        Ok(())
    }

    #[tokio::test]
    async fn approval_required_with_stale_active_turn_is_denied() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "needs approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: Some(true),
                },
            )
            .await?;

        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));
        {
            let mut active = manager.active.lock().await;
            let state = active
                .engines
                .get_mut(&thread.id)
                .context("missing active thread state")?;
            state.active_turn = None;
        }

        harness
            .tx_event
            .send(EngineEvent::ApprovalRequired {
                approval_key: "test_key".to_string(),
                id: "tool_stale".to_string(),
                tool_name: "exec_command".to_string(),
                description: "stale approval".to_string(),
            })
            .await?;

        assert_eq!(
            harness.recv_approval_event().await,
            Some(MockApprovalEvent::Denied {
                id: "tool_stale".to_string(),
            })
        );

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;

        let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn approval_required_awaits_external_decision_allow() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "needs approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));

        harness
            .tx_event
            .send(EngineEvent::ApprovalRequired {
                approval_key: "key1".to_string(),
                id: "tool_external_allow".to_string(),
                tool_name: "exec_command".to_string(),
                description: "external allow".to_string(),
            })
            .await?;

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
            sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(manager.pending_approvals_count(), 1);

        assert!(manager.deliver_external_approval(
            "tool_external_allow",
            ExternalApprovalDecision::Allow { remember: false },
        ));
        assert_eq!(
            harness.recv_approval_event().await,
            Some(MockApprovalEvent::Approved {
                id: "tool_external_allow".to_string(),
            })
        );
        assert_eq!(manager.pending_approvals_count(), 0);

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage::default(),
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn approval_required_external_deny_is_denied() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "needs approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));

        harness
            .tx_event
            .send(EngineEvent::ApprovalRequired {
                approval_key: "key2".to_string(),
                id: "tool_external_deny".to_string(),
                tool_name: "exec_command".to_string(),
                description: "external deny".to_string(),
            })
            .await?;

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
            sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(manager.pending_approvals_count(), 1);

        assert!(manager.deliver_external_approval(
            "tool_external_deny",
            ExternalApprovalDecision::Deny { remember: false },
        ));
        assert_eq!(
            harness.recv_approval_event().await,
            Some(MockApprovalEvent::Denied {
                id: "tool_external_deny".to_string(),
            })
        );

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage::default(),
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn thinking_delta_emits_agent_reasoning_item() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: Some(true),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let mut event_rx = manager.subscribe_events();
        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "show your thinking".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: Some(true),
                },
            )
            .await?;
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));

        harness
            .tx_event
            .send(EngineEvent::ThinkingStarted { index: 0 })
            .await?;
        harness
            .tx_event
            .send(EngineEvent::ThinkingDelta {
                index: 0,
                content: "Let me reason about this.".to_string(),
            })
            .await?;
        harness
            .tx_event
            .send(EngineEvent::ThinkingComplete { index: 0 })
            .await?;
        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage::default(),
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut delta_seen = false;
        let mut completed_seen = false;
        while Instant::now() < deadline && (!delta_seen || !completed_seen) {
            match tokio::time::timeout(Duration::from_millis(200), event_rx.recv()).await {
                Ok(Ok(record)) => {
                    if record.event == "item.delta"
                        && record.payload.get("kind").and_then(|v| v.as_str())
                            == Some("agent_reasoning")
                    {
                        delta_seen = true;
                        assert_eq!(
                            record.payload.get("delta").and_then(|v| v.as_str()),
                            Some("Let me reason about this.")
                        );
                    }
                    if record.event == "item.completed"
                        && record
                            .payload
                            .get("item")
                            .and_then(|v| v.get("kind"))
                            .and_then(|v| v.as_str())
                            == Some("agent_reasoning")
                    {
                        completed_seen = true;
                    }
                }
                _ => break,
            }
        }
        assert!(delta_seen, "expected item.delta with kind=agent_reasoning");
        assert!(
            completed_seen,
            "expected item.completed for the reasoning item"
        );
        Ok(())
    }

    #[tokio::test]
    async fn deliver_external_approval_for_unknown_id_returns_false() {
        let manager = test_manager(test_runtime_dir()).expect("manager");
        assert!(!manager.deliver_external_approval(
            "no_such_approval",
            ExternalApprovalDecision::Allow { remember: false },
        ));
        assert_eq!(manager.pending_approvals_count(), 0);
    }

    #[tokio::test]
    async fn approval_required_remember_flips_thread_auto_approve() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        assert!(!manager.store.load_thread(&thread.id)?.auto_approve);

        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let _turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "needs approval".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));

        harness
            .tx_event
            .send(EngineEvent::ApprovalRequired {
                approval_key: "key3".to_string(),
                id: "tool_remember".to_string(),
                tool_name: "exec_command".to_string(),
                description: "remember=true".to_string(),
            })
            .await?;

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
            sleep(Duration::from_millis(20)).await;
        }
        assert!(manager.deliver_external_approval(
            "tool_remember",
            ExternalApprovalDecision::Allow { remember: true },
        ));
        let _ = harness.recv_approval_event().await;

        assert!(
            manager.store.load_thread(&thread.id)?.auto_approve,
            "remember=true should flip thread auto_approve"
        );

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage::default(),
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn elevation_required_with_stale_active_turn_is_denied() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: Some(true),
                auto_approve: Some(true),
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let mut harness = install_mock_engine(&manager, &thread.id).await;
        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "needs elevation".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: Some(true),
                    auto_approve: Some(true),
                },
            )
            .await?;

        assert!(matches!(
            harness.rx_op.recv().await,
            Some(Op::SendMessage { .. })
        ));
        {
            let mut active = manager.active.lock().await;
            let state = active
                .engines
                .get_mut(&thread.id)
                .context("missing active thread state")?;
            state.active_turn = None;
        }

        harness
            .tx_event
            .send(EngineEvent::ElevationRequired {
                tool_id: "tool_stale_elevated".to_string(),
                tool_name: "exec_command".to_string(),
                command: None,
                denial_reason: "sandbox denied".to_string(),
                blocked_network: false,
                blocked_write: false,
            })
            .await?;

        assert_eq!(
            harness.recv_approval_event().await,
            Some(MockApprovalEvent::Denied {
                id: "tool_stale_elevated".to_string(),
            })
        );

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage {
                    input_tokens: 0,
                    output_tokens: 0,
                    ..Usage::default()
                },
                status: TurnOutcomeStatus::Completed,
                error: None,
            })
            .await?;

        let terminal = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(terminal.status, RuntimeTurnStatus::Completed);
        Ok(())
    }

    #[tokio::test]
    async fn steer_turn_on_active_turn_records_item_and_event() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let mut rx_steer = harness.rx_steer;
        let tx_event = harness.tx_event;
        let (steer_seen_tx, steer_seen_rx) = oneshot::channel::<String>();
        tokio::spawn(async move {
            if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: "engine_turn_steer".to_string(),
                    })
                    .await;
                if let Some(steer) = rx_steer.recv().await {
                    let _ = steer_seen_tx.send(steer);
                }
                let _ = tx_event
                    .send(EngineEvent::MessageStarted { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageDelta {
                        index: 0,
                        content: "steered response".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::MessageComplete { index: 0 })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 8,
                            output_tokens: 9,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                    })
                    .await;
            }
        });

        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "initial".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;

        let steer_text = "add bullet list".to_string();
        let steered_turn = manager
            .steer_turn(
                &thread.id,
                &turn.id,
                SteerTurnRequest {
                    prompt: steer_text.clone(),
                },
            )
            .await?;
        assert_eq!(steered_turn.steer_count, 1);
        let observed_steer = steer_seen_rx
            .await
            .context("driver did not receive steer")?;
        assert_eq!(observed_steer, steer_text);

        let final_turn = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(final_turn.status, RuntimeTurnStatus::Completed);
        assert_eq!(final_turn.steer_count, 1);

        let events = manager.events_since(&thread.id, None)?;
        assert!(events.iter().any(|ev| ev.event == "turn.steered"));
        assert!(events.iter().any(|ev| {
            ev.event == "item.completed"
                && ev
                    .payload
                    .get("item")
                    .and_then(|item| item.get("detail"))
                    .and_then(Value::as_str)
                    == Some("add bullet list")
        }));
        Ok(())
    }

    #[tokio::test]
    async fn compaction_lifecycle_emits_item_events_with_compaction_counts() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        tokio::spawn(async move {
            let mut op_count = 0usize;
            while let Some(op) = rx_op.recv().await {
                match op {
                    Op::SendMessage { .. } => {
                        op_count = op_count.saturating_add(1);
                        let _ = tx_event
                            .send(EngineEvent::TurnStarted {
                                turn_id: "engine_turn_auto".to_string(),
                            })
                            .await;
                        let _ = tx_event
                            .send(EngineEvent::CompactionStarted {
                                id: "auto_compact_1".to_string(),
                                auto: true,
                                message: "auto compact begin".to_string(),
                            })
                            .await;
                        let _ = tx_event
                            .send(EngineEvent::CompactionCompleted {
                                id: "auto_compact_1".to_string(),
                                auto: true,
                                message: "auto compact done".to_string(),
                                messages_before: Some(7),
                                messages_after: Some(3),
                            })
                            .await;
                        let _ = tx_event
                            .send(EngineEvent::TurnComplete {
                                usage: Usage {
                                    input_tokens: 3,
                                    output_tokens: 3,
                                    ..Usage::default()
                                },
                                status: TurnOutcomeStatus::Completed,
                                error: None,
                            })
                            .await;
                    }
                    Op::CompactContext => {
                        op_count = op_count.saturating_add(1);
                        let _ = tx_event
                            .send(EngineEvent::CompactionStarted {
                                id: "manual_compact_1".to_string(),
                                auto: false,
                                message: "manual compact begin".to_string(),
                            })
                            .await;
                        let _ = tx_event
                            .send(EngineEvent::CompactionCompleted {
                                id: "manual_compact_1".to_string(),
                                auto: false,
                                message: "manual compact done".to_string(),
                                messages_before: Some(5),
                                messages_after: Some(2),
                            })
                            .await;
                        let _ = tx_event
                            .send(EngineEvent::TurnComplete {
                                usage: Usage {
                                    input_tokens: 1,
                                    output_tokens: 1,
                                    ..Usage::default()
                                },
                                status: TurnOutcomeStatus::Completed,
                                error: None,
                            })
                            .await;
                    }
                    _ => {}
                }
                if op_count >= 2 {
                    break;
                }
            }
        });

        let auto_turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "trigger auto".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                },
            )
            .await?;
        let auto_turn =
            wait_for_terminal_turn(&manager, &auto_turn.id, Duration::from_secs(2)).await?;
        assert_eq!(auto_turn.status, RuntimeTurnStatus::Completed);

        let manual_turn = manager
            .compact_thread(
                &thread.id,
                CompactThreadRequest {
                    reason: Some("manual request".to_string()),
                },
            )
            .await?;
        let manual_turn =
            wait_for_terminal_turn(&manager, &manual_turn.id, Duration::from_secs(2)).await?;
        assert_eq!(manual_turn.status, RuntimeTurnStatus::Completed);

        let events = manager.events_since(&thread.id, None)?;
        assert!(events.iter().any(|ev| {
            ev.event == "item.started"
                && ev
                    .payload
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("context_compaction")
                && ev.payload.get("auto").and_then(Value::as_bool) == Some(true)
        }));
        assert!(events.iter().any(|ev| {
            ev.event == "item.completed"
                && ev
                    .payload
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("context_compaction")
                && ev.payload.get("auto").and_then(Value::as_bool) == Some(true)
                && ev.payload.get("messages_before").and_then(Value::as_u64) == Some(7)
                && ev.payload.get("messages_after").and_then(Value::as_u64) == Some(3)
        }));
        assert!(events.iter().any(|ev| {
            ev.event == "item.completed"
                && ev
                    .payload
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("context_compaction")
                && ev.payload.get("auto").and_then(Value::as_bool) == Some(false)
                && ev.payload.get("messages_before").and_then(Value::as_u64) == Some(5)
                && ev.payload.get("messages_after").and_then(Value::as_u64) == Some(2)
        }));
        Ok(())
    }

    #[test]
    fn summarize_text_truncates() {
        let out = summarize_text("abcdefghijklmnopqrstuvwxyz", 10);
        assert_eq!(out, "abcdefg...");
    }

    #[test]
    fn approval_decision_requires_auto_approve_and_trust_for_full_access() {
        assert_eq!(
            RuntimeThreadManager::approval_decision(false, false, false),
            RuntimeApprovalDecision::DenyTool
        );
        assert_eq!(
            RuntimeThreadManager::approval_decision(true, false, false),
            RuntimeApprovalDecision::ApproveTool
        );
        assert_eq!(
            RuntimeThreadManager::approval_decision(true, false, true),
            RuntimeApprovalDecision::DenyTool
        );
        assert_eq!(
            RuntimeThreadManager::approval_decision(true, true, true),
            RuntimeApprovalDecision::RetryWithFullAccess
        );
    }

    #[test]
    fn opening_manager_recovers_stale_queued_and_in_progress_work() -> Result<()> {
        let data_dir = test_runtime_dir();
        let manager = test_manager(data_dir.clone())?;
        let started_at = Utc::now() - chrono::Duration::seconds(5);
        let created_at = started_at - chrono::Duration::seconds(1);

        let thread = ThreadRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "thr_restart".to_string(),
            created_at,
            updated_at: created_at,
            model: DEFAULT_TEXT_MODEL.to_string(),
            workspace: PathBuf::from("."),
            mode: "agent".to_string(),
            allow_shell: false,
            trust_mode: false,
            auto_approve: false,
            latest_turn_id: Some("turn_in_progress".to_string()),
            latest_response_bookmark: None,
            archived: false,
            system_prompt: None,
            task_id: None,
            title: None,
            coherence_state: CoherenceState::default(),
        };
        manager.store.save_thread(&thread)?;

        let completed_item = TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "item_completed".to_string(),
            turn_id: "turn_in_progress".to_string(),
            kind: TurnItemKind::Status,
            status: TurnItemLifecycleStatus::Completed,
            summary: "done".to_string(),
            detail: None,
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(started_at),
            ended_at: Some(started_at + chrono::Duration::seconds(1)),
        };
        let in_progress_item = TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "item_in_progress".to_string(),
            turn_id: "turn_in_progress".to_string(),
            kind: TurnItemKind::ToolCall,
            status: TurnItemLifecycleStatus::InProgress,
            summary: "running".to_string(),
            detail: None,
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: Some(started_at),
            ended_at: None,
        };
        let queued_item = TurnItemRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "item_queued".to_string(),
            turn_id: "turn_queued".to_string(),
            kind: TurnItemKind::ToolCall,
            status: TurnItemLifecycleStatus::Queued,
            summary: "queued".to_string(),
            detail: None,
            metadata: None,
            artifact_refs: Vec::new(),
            started_at: None,
            ended_at: None,
        };
        manager.store.save_item(&completed_item)?;
        manager.store.save_item(&in_progress_item)?;
        manager.store.save_item(&queued_item)?;

        manager.store.save_turn(&TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "turn_in_progress".to_string(),
            thread_id: thread.id.clone(),
            status: RuntimeTurnStatus::InProgress,
            input_summary: "hello".to_string(),
            created_at,
            started_at: Some(started_at),
            ended_at: None,
            duration_ms: None,
            usage: None,
            error: None,
            item_ids: vec![completed_item.id.clone(), in_progress_item.id.clone()],
            steer_count: 0,
        })?;
        manager.store.save_turn(&TurnRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            id: "turn_queued".to_string(),
            thread_id: thread.id.clone(),
            status: RuntimeTurnStatus::Queued,
            input_summary: "later".to_string(),
            created_at,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            usage: None,
            error: None,
            item_ids: vec![queued_item.id.clone()],
            steer_count: 0,
        })?;
        drop(manager);

        let recovered = test_manager(data_dir)?;

        let recovered_thread = recovered.store.load_thread(&thread.id)?;
        assert!(recovered_thread.updated_at >= thread.updated_at);

        let recovered_in_progress_turn = recovered.store.load_turn("turn_in_progress")?;
        assert_eq!(
            recovered_in_progress_turn.status,
            RuntimeTurnStatus::Interrupted
        );
        assert_eq!(
            recovered_in_progress_turn.error.as_deref(),
            Some(RUNTIME_RESTART_REASON)
        );
        assert!(recovered_in_progress_turn.ended_at.is_some());
        assert!(
            recovered_in_progress_turn
                .duration_ms
                .is_some_and(|duration| duration >= 5_000)
        );

        let recovered_queued_turn = recovered.store.load_turn("turn_queued")?;
        assert_eq!(recovered_queued_turn.status, RuntimeTurnStatus::Interrupted);
        assert_eq!(
            recovered_queued_turn.error.as_deref(),
            Some(RUNTIME_RESTART_REASON)
        );
        assert!(recovered_queued_turn.ended_at.is_some());
        assert_eq!(recovered_queued_turn.duration_ms, None);

        assert_eq!(
            recovered.store.load_item(&completed_item.id)?.status,
            TurnItemLifecycleStatus::Completed
        );
        let recovered_in_progress_item = recovered.store.load_item(&in_progress_item.id)?;
        assert_eq!(
            recovered_in_progress_item.status,
            TurnItemLifecycleStatus::Interrupted
        );
        assert!(recovered_in_progress_item.ended_at.is_some());

        let recovered_queued_item = recovered.store.load_item(&queued_item.id)?;
        assert_eq!(
            recovered_queued_item.status,
            TurnItemLifecycleStatus::Interrupted
        );
        assert!(recovered_queued_item.ended_at.is_some());

        Ok(())
    }

    #[test]
    fn parse_mode_defaults_to_agent() {
        assert_eq!(parse_mode("unknown"), AppMode::Agent);
        assert_eq!(parse_mode("plan"), AppMode::Plan);
    }

    fn rebind_event(event: &str, agent_id: &str, seq: u64) -> RuntimeEventRecord {
        RuntimeEventRecord {
            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
            seq,
            timestamp: Utc::now(),
            thread_id: "thr_test".to_string(),
            turn_id: Some("turn_test".to_string()),
            item_id: None,
            event: event.to_string(),
            payload: json!({ "agent_id": agent_id }),
        }
    }

    #[test]
    fn collect_agent_rebind_hints_resumes_a_mid_fanout_session() {
        // Mirror what runtime_threads persists during a real fanout: three
        // workers spawned, two finished, one still running when the session
        // was killed. The TUI re-attach must rebuild placeholders for the
        // running worker AND the two completed workers (the fanout card
        // tracks all of them so the dot-grid stays accurate post-resume).
        let events = vec![
            rebind_event("agent.spawned", "agent_a", 1),
            rebind_event("agent.spawned", "agent_b", 2),
            rebind_event("agent.spawned", "agent_c", 3),
            rebind_event("agent.progress", "agent_a", 4),
            rebind_event("agent.completed", "agent_a", 5),
            rebind_event("agent.progress", "agent_b", 6),
            rebind_event("agent.completed", "agent_b", 7),
            rebind_event("agent.progress", "agent_c", 8),
        ];
        let hints = collect_agent_rebind_hints(&events);
        assert_eq!(hints.len(), 3, "every fanout worker must be rebound");
        let by_id: std::collections::BTreeMap<&str, AgentRebindStatus> = hints
            .iter()
            .map(|h| (h.agent_id.as_str(), h.status))
            .collect();
        assert_eq!(by_id.get("agent_a"), Some(&AgentRebindStatus::Completed));
        assert_eq!(by_id.get("agent_b"), Some(&AgentRebindStatus::Completed));
        assert_eq!(
            by_id.get("agent_c"),
            Some(&AgentRebindStatus::InProgress),
            "in-flight worker must rebind in InProgress, not downgrade"
        );
    }

    #[test]
    fn collect_agent_rebind_hints_ignores_unrelated_events() {
        // Status / tool events should not produce phantom hints — only the
        // agent.* family carries the contract we re-bind from.
        let events = vec![
            RuntimeEventRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                seq: 1,
                timestamp: Utc::now(),
                thread_id: "thr".to_string(),
                turn_id: None,
                item_id: None,
                event: "tool.completed".to_string(),
                payload: json!({"name": "read_file"}),
            },
            rebind_event("agent.spawned", "agent_x", 2),
            RuntimeEventRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                seq: 3,
                timestamp: Utc::now(),
                thread_id: "thr".to_string(),
                turn_id: None,
                item_id: None,
                event: "compaction.completed".to_string(),
                payload: json!({"messages_after": 12}),
            },
        ];
        let hints = collect_agent_rebind_hints(&events);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].agent_id, "agent_x");
    }

    #[test]
    fn collect_agent_rebind_hints_does_not_downgrade_completed_to_in_progress() {
        // Out-of-order replay: a stale `agent.progress` arriving after the
        // completed event must NOT clobber the terminal status. This matters
        // when an event log is concatenated from interrupted segments.
        let events = vec![
            rebind_event("agent.spawned", "agent_y", 1),
            rebind_event("agent.completed", "agent_y", 2),
            rebind_event("agent.progress", "agent_y", 3),
        ];
        let hints = collect_agent_rebind_hints(&events);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].status, AgentRebindStatus::Completed);
    }

    /// Helper for the `fork_at_user_message` tests: write a sequence of
    /// (user, assistant) turns under the given thread id. Each turn gets
    /// one UserMessage item carrying `user_text` in `detail` plus one
    /// AgentMessage item. Turn `created_at` is monotonically increasing
    /// so the chronological sort in `list_turns_for_thread` is stable.
    fn seed_turns_with_user_messages(
        manager: &RuntimeThreadManager,
        thread_id: &str,
        user_texts: &[&str],
    ) -> Result<Vec<String>> {
        let mut turn_ids = Vec::new();
        let base = Utc::now();
        for (offset, text) in user_texts.iter().enumerate() {
            let created_at = base + chrono::Duration::milliseconds(offset as i64);
            let turn_id = format!("turn_test_{offset}");
            let user_item_id = format!("item_user_{offset}");
            let asst_item_id = format!("item_asst_{offset}");
            manager.store.save_item(&TurnItemRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                id: user_item_id.clone(),
                turn_id: turn_id.clone(),
                kind: TurnItemKind::UserMessage,
                status: TurnItemLifecycleStatus::Completed,
                summary: (*text).to_string(),
                detail: Some((*text).to_string()),
                metadata: None,
                artifact_refs: Vec::new(),
                started_at: Some(created_at),
                ended_at: Some(created_at),
            })?;
            manager.store.save_item(&TurnItemRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                id: asst_item_id.clone(),
                turn_id: turn_id.clone(),
                kind: TurnItemKind::AgentMessage,
                status: TurnItemLifecycleStatus::Completed,
                summary: format!("reply {offset}"),
                detail: Some(format!("reply {offset}")),
                metadata: None,
                artifact_refs: Vec::new(),
                started_at: Some(created_at),
                ended_at: Some(created_at),
            })?;
            manager.store.save_turn(&TurnRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                id: turn_id.clone(),
                thread_id: thread_id.to_string(),
                status: RuntimeTurnStatus::Completed,
                input_summary: (*text).to_string(),
                created_at,
                started_at: Some(created_at),
                ended_at: Some(created_at),
                duration_ms: Some(0),
                usage: None,
                error: None,
                item_ids: vec![user_item_id, asst_item_id],
                steer_count: 0,
            })?;
            turn_ids.push(turn_id);
        }
        Ok(turn_ids)
    }

    #[tokio::test]
    async fn fork_at_user_message_drops_tail_and_returns_user_text() -> Result<()> {
        // Seed three completed user/assistant turns. Backtracking with
        // depth=0 should drop only the most recent turn ("third") and
        // hand back its original text so the caller can refill the
        // composer.
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        seed_turns_with_user_messages(&manager, &thread.id, &["first", "second", "third"])?;

        let (forked, original_text) = manager.fork_at_user_message(&thread.id, 0).await?;
        assert_eq!(original_text.as_deref(), Some("third"));
        assert_ne!(forked.id, thread.id);

        let forked_turns = manager.store.list_turns_for_thread(&forked.id)?;
        assert_eq!(
            forked_turns.len(),
            2,
            "depth=0 should drop the most recent turn"
        );
        let summaries: Vec<&str> = forked_turns
            .iter()
            .map(|t| t.input_summary.as_str())
            .collect();
        assert_eq!(summaries, vec!["first", "second"]);
        Ok(())
    }

    #[tokio::test]
    async fn fork_at_user_message_depth_one_drops_two_turns() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        seed_turns_with_user_messages(&manager, &thread.id, &["a", "b", "c", "d"])?;

        let (forked, original_text) = manager.fork_at_user_message(&thread.id, 1).await?;
        assert_eq!(original_text.as_deref(), Some("c"));
        let forked_turns = manager.store.list_turns_for_thread(&forked.id)?;
        let summaries: Vec<&str> = forked_turns
            .iter()
            .map(|t| t.input_summary.as_str())
            .collect();
        assert_eq!(summaries, vec!["a", "b"]);
        Ok(())
    }

    #[tokio::test]
    async fn fork_at_user_message_out_of_range_errors() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        seed_turns_with_user_messages(&manager, &thread.id, &["only"])?;

        let err = manager.fork_at_user_message(&thread.id, 5).await.err();
        assert!(err.is_some(), "depth past the end should bail out");
        Ok(())
    }

    #[tokio::test]
    async fn fork_at_user_message_does_not_mutate_source() -> Result<()> {
        // The source thread must be untouched: turns still present, items
        // still present, latest_turn_id still pointing at the original
        // tail. Backtrack creates a sibling, never edits in place.
        let manager = test_manager(test_runtime_dir())?;
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: None,
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
            })
            .await?;
        let turn_ids = seed_turns_with_user_messages(&manager, &thread.id, &["x", "y", "z"])?;

        let _ = manager.fork_at_user_message(&thread.id, 0).await?;

        let source_turns = manager.store.list_turns_for_thread(&thread.id)?;
        assert_eq!(
            source_turns.len(),
            3,
            "source thread must still hold every turn after fork"
        );
        for tid in &turn_ids {
            assert!(
                manager.store.load_turn(tid).is_ok(),
                "turn {tid} must remain on disk"
            );
        }
        Ok(())
    }
}
