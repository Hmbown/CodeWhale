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
use crate::core::engine::{EngineConfig, EngineHandle, spawn_engine};
use crate::core::events::{Event as EngineEvent, TurnOutcomeStatus};
use crate::core::ops::Op;
use crate::models::{
    ContentBlock, Message, SystemPrompt, Usage, auto_compact_default_for_model,
    compaction_threshold_for_model_at_percent,
};
use crate::tools::plan::new_shared_plan_state;
use crate::tools::subagent::SubAgentStatus;
use crate::tools::todo::new_shared_todo_list;
use crate::tui::app::AppMode;
use codewhale_protocol::runtime::{
    DynamicToolCallContent, DynamicToolCallParams, DynamicToolCallResult, DynamicToolSpec,
    TurnEnvironmentParams,
};

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

fn sort_turn_items_by_start(items: &mut [TurnItemRecord]) {
    let fallback = Utc::now();
    items.sort_by(|a, b| {
        let left = a.started_at.unwrap_or(fallback);
        let right = b.started_at.unwrap_or(fallback);
        left.cmp(&right)
    });
}

/// Bumped to 2 for v0.6.6 after live engine semantics changed. The persisted
/// thread/turn/item records did not change shape, but a v1 reader on a v2
/// session should still fail closed rather than silently mis-replay.
const CURRENT_RUNTIME_SCHEMA_VERSION: u32 = 2;
const RUNTIME_RESTART_REASON: &str = "Interrupted by process restart";
const EMPTY_TURN_REASON: &str = "Turn completed without engine output";
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
    /// The session ID associated with this thread. When set, `ensure_engine_loaded`
    /// loads the full message history (including thinking/tool blocks) from the
    /// session file instead of reconstructing from turns (which loses process info).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
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
        sort_turn_items_by_start(&mut out);
        Ok(out)
    }

    pub fn list_items_for_turns_map(
        &self,
        turn_ids: &[String],
    ) -> Result<HashMap<String, Vec<TurnItemRecord>>> {
        if turn_ids.is_empty() {
            return Ok(HashMap::new());
        }

        for turn_id in turn_ids {
            validated_record_id(turn_id, "turn id")?;
        }

        let wanted: HashSet<&str> = turn_ids.iter().map(String::as_str).collect();
        let mut out: HashMap<String, Vec<TurnItemRecord>> = HashMap::new();
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
            if wanted.contains(item.turn_id.as_str()) {
                out.entry(item.turn_id.clone()).or_default().push(item);
            }
        }

        for items in out.values_mut() {
            sort_turn_items_by_start(items);
        }
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
    pub sessions_dir: Option<PathBuf>,
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
            sessions_dir: crate::session_manager::default_sessions_dir().ok(),
            max_active_threads: MAX_ACTIVE_THREADS_DEFAULT,
        }
    }

    #[must_use]
    pub fn with_sessions_dir(mut self, sessions_dir: PathBuf) -> Self {
        self.sessions_dir = Some(sessions_dir);
        self
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    #[serde(default)]
    pub dynamic_tools: Vec<DynamicToolSpec>,
    #[serde(default)]
    pub environments: Vec<TurnEnvironmentParams>,
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
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartTurnRequest {
    pub prompt: String,
    #[serde(default)]
    pub input_summary: Option<String>,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub allow_shell: Option<bool>,
    pub trust_mode: Option<bool>,
    pub auto_approve: Option<bool>,
    #[serde(default)]
    pub dynamic_tools: Vec<DynamicToolSpec>,
    #[serde(default)]
    pub environment_id: Option<String>,
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
    pending_dynamic_tools: Arc<StdMutex<HashMap<String, oneshot::Sender<DynamicToolCallResult>>>>,
}

/// Helper types for `seed_thread_from_messages` — intermediate representation
/// of a turn being built from session messages before persisting as items.
///
/// A single content block extracted from an assistant message.
enum SeedItem {
    Text(String),
    Thinking(String),
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

/// A turn being assembled from session messages.
struct TurnSeed {
    user_text: String,
    items: Vec<SeedItem>,
}

impl TurnSeed {
    fn has_assistant_content(&self) -> bool {
        self.items
            .iter()
            .any(|item| !matches!(item, SeedItem::ToolResult { .. }))
    }
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
            pending_dynamic_tools: Arc::new(StdMutex::new(HashMap::new())),
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
        if let Ok(mut map) = self.pending_dynamic_tools.lock() {
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

    fn register_pending_dynamic_tool(
        &self,
        call_id: &str,
    ) -> oneshot::Receiver<DynamicToolCallResult> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending_dynamic_tools.lock() {
            map.insert(call_id.to_string(), tx);
        }
        rx
    }

    fn cancel_pending_dynamic_tool(&self, call_id: &str) {
        if let Ok(mut map) = self.pending_dynamic_tools.lock() {
            map.remove(call_id);
        }
    }

    pub fn deliver_external_approval(
        &self,
        approval_id: &str,
        decision: ExternalApprovalDecision,
    ) -> bool {
        let sender = match self.pending_approvals.lock() {
            Ok(mut map) => map.remove(approval_id),
            Err(e) => {
                tracing::error!("pending_approvals mutex poisoned: {e}");
                return false;
            }
        };
        match sender {
            Some(tx) => tx.send(decision).is_ok(),
            None => false,
        }
    }

    pub fn deliver_dynamic_tool_result(
        &self,
        call_id: &str,
        result: DynamicToolCallResult,
    ) -> bool {
        let sender = match self.pending_dynamic_tools.lock() {
            Ok(mut map) => map.remove(call_id),
            Err(e) => {
                tracing::error!("pending_dynamic_tools mutex poisoned: {e}");
                return false;
            }
        };
        match sender {
            Some(tx) => tx.send(result).is_ok(),
            None => false,
        }
    }

    pub async fn submit_user_input(
        &self,
        thread_id: &str,
        input_id: &str,
        response: crate::tools::user_input::UserInputResponse,
    ) -> Result<bool> {
        let active = self.active.lock().await;
        let Some(state) = active.engines.get(thread_id) else {
            bail!("thread '{thread_id}' not found");
        };
        state.engine.submit_user_input(input_id, response).await?;
        Ok(true)
    }

    #[allow(dead_code)]
    pub async fn cancel_user_input(&self, thread_id: &str, input_id: &str) -> Result<bool> {
        let active = self.active.lock().await;
        let Some(state) = active.engines.get(thread_id) else {
            bail!("thread '{thread_id}' not found");
        };
        state.engine.cancel_user_input(input_id).await?;
        Ok(true)
    }

    #[allow(dead_code)]
    pub fn pending_approvals_count(&self) -> usize {
        self.pending_approvals
            .lock()
            .map(|map| map.len())
            .unwrap_or(0)
    }

    #[allow(dead_code)]
    pub fn pending_dynamic_tools_count(&self) -> usize {
        self.pending_dynamic_tools
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

    #[cfg(test)]
    pub(crate) fn register_pending_dynamic_tool_for_test(
        &self,
        call_id: &str,
    ) -> oneshot::Receiver<DynamicToolCallResult> {
        self.register_pending_dynamic_tool(call_id)
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

        {
            let mut active = self.active.lock().await;
            if let Some(state) = active.engines.get_mut(thread_id)
                && let Some(turn) = state.active_turn.as_mut()
            {
                turn.auto_approve = true;
            }
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
            session_id: None,
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
            && req.workspace.is_none()
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
        if let Some(workspace) = req.workspace.as_ref()
            && workspace.as_os_str().is_empty()
        {
            bail!("workspace must not be empty");
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
        if let Some(workspace) = req.workspace
            && thread.workspace != workspace
        {
            changes.insert("workspace".to_string(), json!(workspace));
            thread.workspace = workspace;
        }

        if !changes.is_empty() {
            let workspace_changed = changes.contains_key("workspace");
            if workspace_changed {
                self.ensure_thread_has_no_active_turn(&thread.id).await?;
            }

            thread.updated_at = Utc::now();
            self.store.save_thread(&thread)?;
            if workspace_changed {
                self.evict_cached_engine(&thread.id).await;
            }
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

    /// Link a session to a thread so that `ensure_engine_loaded` can restore
    /// the full message history (including thinking/tool blocks) from the
    /// session file instead of reconstructing from turns.
    pub async fn set_thread_session_id(&self, thread_id: &str, session_id: &str) -> Result<()> {
        let mut thread = self.get_thread(thread_id).await?;
        if thread.session_id.as_deref() == Some(session_id) {
            return Ok(());
        }
        thread.session_id = Some(session_id.to_string());
        thread.updated_at = Utc::now();
        self.store.save_thread(&thread)?;
        self.emit_event(
            thread_id,
            None,
            None,
            "thread.updated",
            json!({ "thread": thread, "changes": { "session_id": session_id } }),
        )
        .await?;
        Ok(())
    }

    async fn ensure_thread_has_no_active_turn(&self, thread_id: &str) -> Result<()> {
        let active = self.active.lock().await;
        if active
            .engines
            .get(thread_id)
            .and_then(|state| state.active_turn.as_ref())
            .is_some()
        {
            bail!("workspace cannot be changed while the thread has an active turn");
        }
        Ok(())
    }

    async fn evict_cached_engine(&self, thread_id: &str) {
        let engine = {
            let mut active = self.active.lock().await;
            active.lru.retain(|id| id != thread_id);
            active.engines.remove(thread_id).map(|state| state.engine)
        };
        if let Some(engine) = engine {
            let _ = engine.send(Op::Shutdown).await;
        }
    }

    pub async fn get_thread_detail(&self, id: &str) -> Result<ThreadDetail> {
        let thread = self.get_thread(id).await?;
        let turns = self.store.list_turns_for_thread(id)?;
        let turn_ids: Vec<String> = turns.iter().map(|turn| turn.id.clone()).collect();
        let mut items_by_turn = self.store.list_items_for_turns_map(&turn_ids)?;
        let mut items = Vec::new();
        for turn in &turns {
            if let Some(mut turn_items) = items_by_turn.remove(&turn.id) {
                items.append(&mut turn_items);
            }
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
    ///
    /// Unlike the old text-only implementation, this preserves all content
    /// block types (thinking, tool_use, tool_result, etc.) as separate turn
    /// items so that `loadHistory` in the GUI can reconstruct the full
    /// conversation including process information.
    pub async fn seed_thread_from_messages(
        &self,
        thread_id: &str,
        messages: &[Message],
    ) -> Result<()> {
        let mut thread = self.get_thread(thread_id).await?;
        let now = Utc::now();

        // Group messages into turns. A turn starts with a user message and
        // includes all subsequent assistant messages (which may contain
        // thinking, tool_use, tool_result blocks) until the next user message.
        //
        // Consecutive user messages (e.g. parallel tool call results) are
        // merged into the same turn to avoid violating LLM provider contracts
        // that forbid back-to-back user messages.
        let mut turns: Vec<TurnSeed> = Vec::new();
        let mut current_turn: Option<TurnSeed> = None;

        for msg in messages {
            match msg.role.as_str() {
                "user" => {
                    // Only flush when the current turn has assistant content;
                    // otherwise merge consecutive user messages into one turn.
                    if current_turn
                        .as_ref()
                        .is_some_and(TurnSeed::has_assistant_content)
                    {
                        turns.push(current_turn.take().unwrap());
                    }
                    let turn = current_turn.get_or_insert_with(|| TurnSeed {
                        user_text: String::new(),
                        items: Vec::new(),
                    });
                    // Extract text from user message content blocks.
                    // Tool result blocks in user messages are part of the
                    // tool loop and should be stored as tool_call items.
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                                if !turn.user_text.is_empty() {
                                    turn.user_text.push('\n');
                                }
                                turn.user_text.push_str(text);
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                                ..
                            } => {
                                turn.items.push(SeedItem::ToolResult {
                                    tool_use_id: tool_use_id.clone(),
                                    content: content.clone(),
                                    is_error: is_error.unwrap_or(false),
                                });
                            }
                            // Other block types in user messages are rare;
                            // skip them gracefully.
                            _ => {}
                        }
                    }
                }
                "assistant" => {
                    // If no current turn exists (e.g. session starts with
                    // an assistant message), create a placeholder turn.
                    let turn = current_turn.get_or_insert_with(|| TurnSeed {
                        user_text: String::new(),
                        items: Vec::new(),
                    });
                    for block in &msg.content {
                        match block {
                            ContentBlock::Text { text, .. } if !text.trim().is_empty() => {
                                turn.items.push(SeedItem::Text(text.clone()));
                            }
                            ContentBlock::Thinking { thinking, .. }
                                if !thinking.trim().is_empty() =>
                            {
                                turn.items.push(SeedItem::Thinking(thinking.clone()));
                            }
                            ContentBlock::ToolUse {
                                id, name, input, ..
                            } => {
                                turn.items.push(SeedItem::ToolUse {
                                    id: id.clone(),
                                    name: name.clone(),
                                    input: input.clone(),
                                });
                            }
                            ContentBlock::ServerToolUse {
                                id, name, input, ..
                            } => {
                                turn.items.push(SeedItem::ToolUse {
                                    id: id.clone(),
                                    name: name.clone(),
                                    input: input.clone(),
                                });
                            }
                            // Skip other block types (image_url, etc.)
                            _ => {}
                        }
                    }
                }
                // System messages and other roles are ignored for turn seeding.
                _ => {}
            }
        }
        // Flush the last turn.
        if let Some(t) = current_turn.take() {
            turns.push(t);
        }

        for (offset, turn_seed) in turns.into_iter().enumerate() {
            let created_at = now + chrono::Duration::seconds(offset as i64);
            let turn_id = format!("turn_{}", &Uuid::new_v4().to_string()[..8]);
            let summary =
                crate::utils::truncate_with_ellipsis(&turn_seed.user_text, SUMMARY_LIMIT, "...");
            let mut item_ids = Vec::new();
            let mut item_offset_ms = 0_i64;

            let mut next_item_time = || {
                let ts = created_at + chrono::Duration::milliseconds(item_offset_ms);
                item_offset_ms += 1;
                ts
            };

            // Save user message item.
            if !turn_seed.user_text.is_empty() {
                let item_time = next_item_time();
                let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                self.store.save_item(&TurnItemRecord {
                    schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                    id: item_id.clone(),
                    turn_id: turn_id.clone(),
                    kind: TurnItemKind::UserMessage,
                    status: TurnItemLifecycleStatus::Completed,
                    summary: summary.clone(),
                    detail: Some(turn_seed.user_text.clone()),
                    metadata: None,
                    artifact_refs: Vec::new(),
                    started_at: Some(item_time),
                    ended_at: Some(item_time),
                })?;
                item_ids.push(item_id);
            }

            // Save assistant content items in order.
            for seed_item in &turn_seed.items {
                let item_time = next_item_time();
                let item_id = format!("item_{}", &Uuid::new_v4().to_string()[..8]);
                match seed_item {
                    SeedItem::Text(text) => {
                        let asst_summary = if text.len() > SUMMARY_LIMIT {
                            crate::utils::truncate_with_ellipsis(text, SUMMARY_LIMIT, "...")
                        } else {
                            text.clone()
                        };
                        self.store.save_item(&TurnItemRecord {
                            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                            id: item_id.clone(),
                            turn_id: turn_id.clone(),
                            kind: TurnItemKind::AgentMessage,
                            status: TurnItemLifecycleStatus::Completed,
                            summary: asst_summary,
                            detail: Some(text.clone()),
                            metadata: None,
                            artifact_refs: Vec::new(),
                            started_at: Some(item_time),
                            ended_at: Some(item_time),
                        })?;
                    }
                    SeedItem::Thinking(thinking) => {
                        let thinking_summary = if thinking.len() > SUMMARY_LIMIT {
                            crate::utils::truncate_with_ellipsis(thinking, SUMMARY_LIMIT, "...")
                        } else {
                            thinking.clone()
                        };
                        self.store.save_item(&TurnItemRecord {
                            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                            id: item_id.clone(),
                            turn_id: turn_id.clone(),
                            kind: TurnItemKind::AgentReasoning,
                            status: TurnItemLifecycleStatus::Completed,
                            summary: thinking_summary,
                            detail: Some(thinking.clone()),
                            metadata: None,
                            artifact_refs: Vec::new(),
                            started_at: Some(item_time),
                            ended_at: Some(item_time),
                        })?;
                    }
                    SeedItem::ToolUse {
                        id: tool_id,
                        name,
                        input,
                    } => {
                        let input_str =
                            serde_json::to_string(input).unwrap_or_else(|_| input.to_string());
                        let tool_summary = format!("{name}({})", {
                            let s = &input_str;
                            if s.len() > 80 {
                                crate::utils::truncate_with_ellipsis(s, 80, "...")
                            } else {
                                s.clone()
                            }
                        });
                        self.store.save_item(&TurnItemRecord {
                            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                            id: item_id.clone(),
                            turn_id: turn_id.clone(),
                            kind: TurnItemKind::ToolCall,
                            status: TurnItemLifecycleStatus::Completed,
                            summary: tool_summary,
                            detail: Some(input_str),
                            metadata: Some(serde_json::Value::Object(
                                serde_json::json!({
                                    "tool_use_id": tool_id,
                                    "tool_name": name,
                                })
                                .as_object()
                                .unwrap()
                                .clone(),
                            )),
                            artifact_refs: Vec::new(),
                            started_at: Some(item_time),
                            ended_at: Some(item_time),
                        })?;
                    }
                    SeedItem::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => {
                        let result_summary = if content.len() > SUMMARY_LIMIT {
                            crate::utils::truncate_with_ellipsis(content, SUMMARY_LIMIT, "...")
                        } else {
                            content.clone()
                        };
                        self.store.save_item(&TurnItemRecord {
                            schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                            id: item_id.clone(),
                            turn_id: turn_id.clone(),
                            kind: TurnItemKind::ToolCall,
                            status: if *is_error {
                                TurnItemLifecycleStatus::Failed
                            } else {
                                TurnItemLifecycleStatus::Completed
                            },
                            summary: result_summary,
                            detail: Some(content.clone()),
                            metadata: Some(serde_json::Value::Object(
                                serde_json::json!({
                                    "tool_result_for": tool_use_id,
                                    "is_error": is_error,
                                })
                                .as_object()
                                .unwrap()
                                .clone(),
                            )),
                            artifact_refs: Vec::new(),
                            started_at: Some(item_time),
                            ended_at: Some(item_time),
                        })?;
                    }
                }
                item_ids.push(item_id);
            }

            // Only create a turn if there's content.
            if !item_ids.is_empty() {
                self.store.save_turn(&TurnRecord {
                    schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                    id: turn_id.clone(),
                    thread_id: thread_id.to_string(),
                    status: RuntimeTurnStatus::Completed,
                    input_summary: summary,
                    created_at,
                    started_at: Some(created_at),
                    ended_at: Some(created_at),
                    duration_ms: Some(0),
                    usage: None,
                    error: None,
                    item_ids,
                    steer_count: 0,
                })?;

                thread.latest_turn_id = Some(turn_id);
                thread.updated_at = now;
            }
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
        let (provider, model, reasoning_effort) = if auto_model {
            let selection = crate::model_routing::resolve_auto_route_with_inventory(
                &self.config,
                &prompt,
                "",
                "auto",
                "auto",
            )
            .await?;
            (
                selection.provider,
                selection.model,
                selection
                    .reasoning_effort
                    .map(|effort| effort.as_setting().to_string()),
            )
        } else {
            (self.config.api_provider(), requested_model, None)
        };
        let allow_shell = req.allow_shell.unwrap_or(thread.allow_shell);
        let trust_mode = req.trust_mode.unwrap_or(thread.trust_mode);
        let auto_approve = req.auto_approve.unwrap_or(thread.auto_approve);
        let show_thinking = crate::settings::Settings::load()
            .unwrap_or_default()
            .show_thinking;

        engine
            .send(Op::SendMessage {
                content: prompt,
                mode,
                provider: Some(provider),
                model: model.clone(),
                goal_objective: None,
                goal_token_budget: None,
                goal_status: crate::tools::goal::GoalStatus::Active,
                reasoning_effort,
                reasoning_effort_auto: auto_model,
                auto_model,
                allow_shell,
                trust_mode,
                auto_approve,
                translation_enabled: false,
                show_thinking,
                allowed_tools: None,
                dynamic_tools: req.dynamic_tools,
                hook_executor: None,
                approval_mode: if auto_approve {
                    crate::tui::approval::ApprovalMode::Auto
                } else {
                    crate::tui::approval::ApprovalMode::Suggest
                },
                verbosity: self.config.verbosity.clone(),
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
        let settings = crate::settings::Settings::load().unwrap_or_default();
        let auto_compact_enabled =
            if crate::settings::Settings::auto_compact_explicitly_configured() {
                settings.auto_compact
            } else {
                auto_compact_default_for_model(&thread.model)
            };
        let compaction = CompactionConfig {
            enabled: auto_compact_enabled,
            model: thread.model.clone(),
            token_threshold: compaction_threshold_for_model_at_percent(
                &thread.model,
                settings.auto_compact_threshold_percent,
            ),
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
            skills_scan_codewhale_only: self.config.skills_config().scan_codewhale_only(),
            instructions: self
                .config
                .instructions_paths()
                .into_iter()
                .map(Into::into)
                .collect(),
            project_context_pack_enabled: self.config.project_context_pack_enabled(),
            translation_enabled: false,
            show_thinking: settings.show_thinking,
            max_steps: 100,
            max_subagents: self.config.max_subagents().clamp(1, MAX_SUBAGENTS),
            launch_concurrency: self.config.launch_concurrency(),
            features: self.config.features(),
            compaction,
            todos: new_shared_todo_list(),
            plan_state: new_shared_plan_state(),
            goal_state: crate::tools::goal::new_shared_goal_state(),
            max_spawn_depth: self.config.subagent_max_spawn_depth(),
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
                dynamic_tool_executor: Some(Arc::new(self.clone())),
                shell_manager: None,
                hook_executor: None,
                handle_store: crate::tools::handle::new_shared_handle_store(),
                rlm_sessions: crate::rlm::session::new_shared_rlm_session_store(),
            },
            subagent_model_overrides: self.config.subagent_model_overrides(),
            subagent_api_timeout: std::time::Duration::from_secs(
                self.config.subagent_api_timeout_secs(),
            ),
            stream_chunk_timeout: std::time::Duration::from_secs(
                self.config.stream_chunk_timeout_secs(),
            ),
            subagent_heartbeat_timeout: std::time::Duration::from_secs(
                self.config.subagent_heartbeat_timeout_secs(),
            ),
            prefer_bwrap: self.config.prefer_bwrap.unwrap_or(false),
            memory_enabled: self.config.memory_enabled(),
            memory_path: self.config.memory_path(),
            speech_output_dir: self.config.speech_output_dir(),
            vision_config: self.config.vision_model_config(),
            strict_tool_mode: self.config.strict_tool_mode.unwrap_or(false),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            allowed_tools: None,
            disallowed_tools: None,
            hook_executor: None,
            locale_tag: crate::localization::resolve_locale(&settings.locale)
                .tag()
                .to_string(),
            workshop: self.config.workshop.clone(),
            search_provider: self.config.search_provider(),
            search_api_key: self.config.search.as_ref().and_then(|s| s.api_key.clone()),
            search_base_url: self.config.search.as_ref().and_then(|s| s.base_url.clone()),
            tools_always_load: self.config.tools_always_load(),
            tools: self.config.tools.clone(),
            verbosity: self.config.verbosity.clone(),
            workspace_follow_symlinks: settings.workspace_follow_symlinks,
            exec_policy_engine: self.config.exec_policy_engine.clone(),
        };

        let engine = spawn_engine(engine_cfg, &self.config);

        // When the thread has an associated session, load the full message history
        // (including thinking/tool blocks) from the session file. This preserves
        // process information that `reconstruct_messages_from_turns` would lose.
        let session_messages = if let Some(ref sid) = thread.session_id {
            let fallback = || -> Result<Vec<Message>> {
                let turns = self.store.list_turns_for_thread(&thread.id)?;
                self.reconstruct_messages_from_turns(&turns)
            };

            self.manager_cfg
                .sessions_dir
                .clone()
                .ok_or_else(|| anyhow!("No sessions dir configured"))
                .and_then(|dir| {
                    crate::session_manager::SessionManager::new(dir).map_err(anyhow::Error::from)
                })
                .and_then(|manager| manager.load_session(sid).map_err(anyhow::Error::from))
                .map(|session| session.messages)
                .map_or_else(
                    |e| {
                        tracing::warn!(
                            "Failed to load session {sid} for thread {}: {e}; \
                             falling back to turn reconstruction",
                            thread.id,
                        );
                        fallback()
                    },
                    Ok,
                )?
        } else {
            let turns = self.store.list_turns_for_thread(&thread.id)?;
            self.reconstruct_messages_from_turns(&turns)?
        };
        let sys_prompt = thread
            .system_prompt
            .as_ref()
            .map(|s| SystemPrompt::Text(s.clone()));
        if !session_messages.is_empty() || sys_prompt.is_some() {
            engine
                .send(Op::SyncSession {
                    session_id: thread.session_id.clone(),
                    messages: session_messages,
                    system_prompt: sys_prompt,
                    system_prompt_override: thread.system_prompt.is_some(),
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

    /// Get the engine handle for a thread, loading it if necessary.
    /// Public wrapper around the private `ensure_engine_loaded`.
    pub async fn get_engine(&self, thread_id: &str) -> Result<EngineHandle> {
        let thread = self.get_thread(thread_id).await?;
        self.ensure_engine_loaded(&thread).await
    }

    fn reconstruct_messages_from_turns(&self, turns: &[TurnRecord]) -> Result<Vec<Message>> {
        let mut messages = Vec::new();
        for turn in turns {
            let items = self.store.list_items_for_turn(&turn.id)?;
            let mut user_blocks: Vec<ContentBlock> = Vec::new();
            // Collect content blocks for the current assistant message.
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            let flush_user = |blocks: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
                if !blocks.is_empty() {
                    msgs.push(Message {
                        role: "user".to_string(),
                        content: std::mem::take(blocks),
                    });
                }
            };
            let flush_assistant = |blocks: &mut Vec<ContentBlock>, msgs: &mut Vec<Message>| {
                if !blocks.is_empty() {
                    msgs.push(Message {
                        role: "assistant".to_string(),
                        content: std::mem::take(blocks),
                    });
                }
            };
            for item in items {
                match item.kind {
                    TurnItemKind::UserMessage => {
                        flush_assistant(&mut assistant_blocks, &mut messages);
                        let text = item.detail.unwrap_or(item.summary);
                        if !text.trim().is_empty() {
                            user_blocks.push(ContentBlock::Text {
                                text,
                                cache_control: None,
                            });
                        }
                    }
                    TurnItemKind::AgentMessage => {
                        flush_user(&mut user_blocks, &mut messages);
                        let text = item.detail.unwrap_or(item.summary);
                        if !text.trim().is_empty() {
                            assistant_blocks.push(ContentBlock::Text {
                                text,
                                cache_control: None,
                            });
                        }
                    }
                    TurnItemKind::AgentReasoning => {
                        flush_user(&mut user_blocks, &mut messages);
                        let thinking = item.detail.unwrap_or(item.summary);
                        if !thinking.trim().is_empty() {
                            assistant_blocks.push(ContentBlock::Thinking {
                                thinking,
                                signature: None,
                            });
                        }
                    }
                    TurnItemKind::ToolCall => {
                        let meta = item.metadata.as_ref();
                        let is_tool_result = meta.and_then(|m| m.get("tool_result_for")).is_some();
                        if is_tool_result {
                            flush_assistant(&mut assistant_blocks, &mut messages);
                            let tool_use_id = meta
                                .and_then(|m| m.get("tool_result_for"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let content = item.detail.unwrap_or_default();
                            let is_error = meta
                                .and_then(|m| m.get("is_error"))
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            user_blocks.push(ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error: if is_error { Some(true) } else { None },
                                content_blocks: None,
                            });
                        } else {
                            flush_user(&mut user_blocks, &mut messages);
                            let tool_use_id = meta
                                .and_then(|m| m.get("tool_use_id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let tool_name = meta
                                .and_then(|m| m.get("tool_name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let input_str = item.detail.unwrap_or_default();
                            let input: serde_json::Value =
                                serde_json::from_str(&input_str).unwrap_or(serde_json::Value::Null);
                            assistant_blocks.push(ContentBlock::ToolUse {
                                id: tool_use_id,
                                name: tool_name,
                                input,
                                caller: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            flush_assistant(&mut assistant_blocks, &mut messages);
            flush_user(&mut user_blocks, &mut messages);
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
        let mut saw_engine_activity = false;

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

            if !matches!(
                &event,
                EngineEvent::TurnStarted { .. } | EngineEvent::TurnComplete { .. }
            ) {
                saw_engine_activity = true;
            }

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
                EngineEvent::AgentSpawned { id, prompt, .. } => {
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
                EngineEvent::AgentProgress { id, status, .. } => {
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
                    intent_summary,
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
                            "intent_summary": intent_summary,
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
                EngineEvent::UserInputRequired { id, request } => {
                    self.emit_event(
                        &thread_id,
                        Some(&turn_id),
                        None,
                        "user_input.required",
                        json!({
                            "id": id,
                            "request": request,
                        }),
                    )
                    .await?;
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
                    ..
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

        if turn_status == RuntimeTurnStatus::Completed && !saw_engine_activity {
            turn_status = RuntimeTurnStatus::Failed;
            turn_error = Some(EMPTY_TURN_REASON.to_string());
            let item = TurnItemRecord {
                schema_version: CURRENT_RUNTIME_SCHEMA_VERSION,
                id: format!("item_{}", &Uuid::new_v4().to_string()[..8]),
                turn_id: turn_id.clone(),
                kind: TurnItemKind::Error,
                status: TurnItemLifecycleStatus::Failed,
                summary: EMPTY_TURN_REASON.to_string(),
                detail: Some(EMPTY_TURN_REASON.to_string()),
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

    async fn active_turn_id(&self, thread_id: &str) -> Option<String> {
        let active = self.active.lock().await;
        active
            .engines
            .get(thread_id)?
            .active_turn
            .as_ref()
            .map(|turn| turn.turn_id.clone())
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

fn dynamic_tool_result_text(content: &[DynamicToolCallContent]) -> String {
    content
        .iter()
        .map(|item| match item {
            DynamicToolCallContent::InputText { text } => text.clone(),
            DynamicToolCallContent::InputImage { image_url } => format!("[image] {image_url}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[async_trait::async_trait]
impl crate::tools::spec::DynamicToolExecutor for RuntimeThreadManager {
    async fn execute_dynamic_tool(
        &self,
        thread_id: Option<String>,
        namespace: Option<String>,
        name: String,
        input: Value,
    ) -> std::result::Result<crate::tools::spec::ToolResult, crate::tools::spec::ToolError> {
        let thread_id = thread_id.ok_or_else(|| {
            crate::tools::spec::ToolError::not_available(format!(
                "runtime dynamic tool '{name}' has no active thread"
            ))
        })?;
        let turn_id = self.active_turn_id(&thread_id).await.ok_or_else(|| {
            crate::tools::spec::ToolError::not_available(format!(
                "runtime dynamic tool '{name}' has no active turn"
            ))
        })?;
        let call_id = format!("call_{}", &Uuid::new_v4().to_string()[..8]);
        let rx = self.register_pending_dynamic_tool(&call_id);

        let params = DynamicToolCallParams {
            thread_id: thread_id.clone(),
            turn_id: turn_id.clone(),
            call_id: call_id.clone(),
            namespace,
            tool: name.clone(),
            arguments: input,
        };
        if let Err(err) = self
            .emit_event(
                &thread_id,
                Some(&turn_id),
                None,
                "tool_call.requested",
                json!(params),
            )
            .await
        {
            self.cancel_pending_dynamic_tool(&call_id);
            return Err(crate::tools::spec::ToolError::execution_failed(format!(
                "failed to emit runtime dynamic tool request for '{name}': {err}"
            )));
        }

        match tokio::time::timeout(APPROVAL_DECISION_TIMEOUT, rx).await {
            Ok(Ok(result)) => {
                let text = dynamic_tool_result_text(&result.content);
                if result.success {
                    Ok(crate::tools::spec::ToolResult::success(text))
                } else {
                    Ok(crate::tools::spec::ToolResult::error(if text.is_empty() {
                        "dynamic tool failed".to_string()
                    } else {
                        text
                    }))
                }
            }
            Ok(Err(_recv_err)) => Err(crate::tools::spec::ToolError::execution_failed(format!(
                "runtime dynamic tool '{name}' result channel closed"
            ))),
            Err(_timeout) => {
                self.cancel_pending_dynamic_tool(&call_id);
                Err(crate::tools::spec::ToolError::Timeout {
                    seconds: APPROVAL_DECISION_TIMEOUT.as_secs(),
                })
            }
        }
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
    use crate::session_manager::{SessionManager, create_saved_session_with_id_and_mode};
    use std::fs;
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
            sessions_dir: None,
            max_active_threads: 4,
        }
    }

    fn test_manager_config_with_sessions_dir(
        data_dir: PathBuf,
        sessions_dir: PathBuf,
    ) -> RuntimeThreadManagerConfig {
        RuntimeThreadManagerConfig {
            task_data_dir: data_dir.clone(),
            data_dir,
            sessions_dir: Some(sessions_dir),
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

    fn test_manager_with_sessions_dir(
        data_dir: PathBuf,
        sessions_dir: PathBuf,
    ) -> Result<RuntimeThreadManager> {
        RuntimeThreadManager::open(
            Config::default(),
            PathBuf::from("."),
            test_manager_config_with_sessions_dir(data_dir, sessions_dir),
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
            session_id: None,
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
                ..Default::default()
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
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 10,
                            output_tokens: 12,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                        tool_catalog: None,
                        base_url: None,
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
                    ..Default::default()
                },
            )
            .await?;
        let completed = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(completed.status, RuntimeTurnStatus::Completed);

        drop(manager);

        let reopened = test_manager(runtime_dir)?;
        let detail = reopened.get_thread_detail(&thread.id).await?;
        assert_eq!(detail.thread.id, thread.id);
        assert_eq!(detail.turns.len(), 1);
        assert!(detail.latest_seq >= 1);
        assert!(!detail.items.is_empty());
        let events = reopened.events_since(&thread.id, None)?;
        assert!(
            events.iter().any(|ev| ev.event == "turn.completed"),
            "expected turn.completed event after restart"
        );
        Ok(())
    }

    #[tokio::test]
    async fn completed_turn_without_engine_output_fails() -> Result<()> {
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
                ..Default::default()
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        let tx_event = harness.tx_event;
        tokio::spawn(async move {
            if matches!(rx_op.recv().await, Some(Op::SendMessage { .. })) {
                let _ = tx_event
                    .send(EngineEvent::TurnStarted {
                        turn_id: "engine_empty_turn".to_string(),
                    })
                    .await;
                let _ = tx_event
                    .send(EngineEvent::TurnComplete {
                        usage: Usage {
                            input_tokens: 10,
                            output_tokens: 0,
                            ..Usage::default()
                        },
                        status: TurnOutcomeStatus::Completed,
                        error: None,
                        tool_catalog: None,
                        base_url: None,
                    })
                    .await;
            }
        });

        let turn = manager
            .start_turn(
                &thread.id,
                StartTurnRequest {
                    prompt: "empty turn".to_string(),
                    input_summary: None,
                    model: None,
                    mode: None,
                    allow_shell: None,
                    trust_mode: None,
                    auto_approve: None,
                    ..Default::default()
                },
            )
            .await?;

        let failed = wait_for_terminal_turn(&manager, &turn.id, Duration::from_secs(2)).await?;
        assert_eq!(failed.status, RuntimeTurnStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some(EMPTY_TURN_REASON));

        let events = manager.events_since(&thread.id, None)?;
        assert!(events.iter().any(|ev| {
            ev.event == "item.failed"
                && ev
                    .payload
                    .get("item")
                    .and_then(|item| item.get("kind"))
                    .and_then(Value::as_str)
                    == Some("error")
        }));
        assert!(events.iter().any(|ev| {
            ev.event == "turn.completed"
                && ev
                    .payload
                    .get("turn")
                    .and_then(|turn| turn.get("status"))
                    .and_then(Value::as_str)
                    == Some("failed")
        }));
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
                ..Default::default()
            })
            .await?;

        assert!(!thread.auto_approve);
        Ok(())
    }

    #[tokio::test]
    async fn update_thread_workspace_persists_event_and_evicts_idle_engine() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let old_workspace = std::env::temp_dir().join("codewhale-runtime-old-workspace");
        let new_workspace = std::env::temp_dir().join("codewhale-runtime-new-workspace");
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: Some(old_workspace.clone()),
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
                ..Default::default()
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;

        let updated = manager
            .update_thread(
                &thread.id,
                UpdateThreadRequest {
                    workspace: Some(new_workspace.clone()),
                    ..UpdateThreadRequest::default()
                },
            )
            .await?;

        assert_eq!(updated.workspace, new_workspace);
        assert_eq!(
            manager.store.load_thread(&thread.id)?.workspace,
            new_workspace
        );
        {
            let active = manager.active.lock().await;
            assert!(
                !active.engines.contains_key(&thread.id),
                "workspace changes must evict the stale cached engine"
            );
            assert!(!active.lru.iter().any(|id| id == &thread.id));
        }

        match tokio::time::timeout(Duration::from_secs(1), rx_op.recv()).await {
            Ok(Some(Op::Shutdown)) => {}
            other => panic!("expected cached engine shutdown, got {other:?}"),
        }

        let events = manager.events_since(&thread.id, None)?;
        let event = events
            .iter()
            .rev()
            .find(|event| event.event == "thread.updated")
            .expect("thread.updated event");
        let workspace_value = serde_json::to_value(&updated.workspace)?;
        assert_eq!(
            event
                .payload
                .get("changes")
                .and_then(|changes| changes.get("workspace")),
            Some(&workspace_value)
        );
        Ok(())
    }

    #[tokio::test]
    async fn update_thread_workspace_rejects_empty_path() -> Result<()> {
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
                ..Default::default()
            })
            .await?;

        let err = manager
            .update_thread(
                &thread.id,
                UpdateThreadRequest {
                    workspace: Some(PathBuf::new()),
                    ..UpdateThreadRequest::default()
                },
            )
            .await
            .expect_err("empty workspace must be rejected");
        assert!(format!("{err:#}").contains("workspace must not be empty"));
        Ok(())
    }

    #[tokio::test]
    async fn update_thread_workspace_rejects_active_turn() -> Result<()> {
        let manager = test_manager(test_runtime_dir())?;
        let old_workspace = std::env::temp_dir().join("codewhale-runtime-active-old");
        let new_workspace = std::env::temp_dir().join("codewhale-runtime-active-new");
        let thread = manager
            .create_thread(CreateThreadRequest {
                model: None,
                workspace: Some(old_workspace.clone()),
                mode: None,
                allow_shell: None,
                trust_mode: None,
                auto_approve: None,
                archived: false,
                system_prompt: None,
                task_id: None,
                ..Default::default()
            })
            .await?;

        let harness = install_mock_engine(&manager, &thread.id).await;
        let mut rx_op = harness.rx_op;
        {
            let mut active = manager.active.lock().await;
            let state = active.engines.get_mut(&thread.id).expect("mock engine");
            state.active_turn = Some(ActiveTurnState {
                turn_id: "turn_live".to_string(),
                interrupt_requested: false,
                auto_approve: false,
                trust_mode: false,
            });
        }

        let err = manager
            .update_thread(
                &thread.id,
                UpdateThreadRequest {
                    workspace: Some(new_workspace),
                    ..UpdateThreadRequest::default()
                },
            )
            .await
            .expect_err("workspace update during active turn must fail");

        assert!(format!("{err:#}").contains("active turn"));
        assert_eq!(
            manager.store.load_thread(&thread.id)?.workspace,
            old_workspace
        );
        {
            let active = manager.active.lock().await;
            assert!(
                active.engines.contains_key(&thread.id),
                "active engine should stay cached after rejected update"
            );
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), rx_op.recv())
                .await
                .is_err(),
            "rejected workspace update must not shut down the active engine"
        );
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
                ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                        tool_catalog: None,
                        base_url: None,
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
                    ..Default::default()
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
                    ..Default::default()
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
    async fn get_thread_detail_batches_items_by_turn_without_losing_order() -> Result<()> {
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
                ..Default::default()
            })
            .await?;

        let base = Utc::now();
        let mut first_turn = sample_turn(
            &thread.id,
            "turn_detail_batch_first",
            RuntimeTurnStatus::Completed,
        );
        first_turn.created_at = base;
        let mut second_turn = sample_turn(
            &thread.id,
            "turn_detail_batch_second",
            RuntimeTurnStatus::Completed,
        );
        second_turn.created_at = base + chrono::Duration::seconds(1);
        manager.store.save_turn(&first_turn)?;
        manager.store.save_turn(&second_turn)?;

        let mut first_late = sample_item(
            &first_turn.id,
            "item_detail_first_late",
            TurnItemLifecycleStatus::Completed,
        );
        first_late.started_at = Some(base + chrono::Duration::seconds(5));
        let mut first_early = sample_item(
            &first_turn.id,
            "item_detail_first_early",
            TurnItemLifecycleStatus::Completed,
        );
        first_early.started_at = Some(base + chrono::Duration::seconds(1));
        let mut second_item = sample_item(
            &second_turn.id,
            "item_detail_second",
            TurnItemLifecycleStatus::Completed,
        );
        second_item.started_at = Some(base + chrono::Duration::seconds(2));
        let unrelated = sample_item(
            "turn_detail_batch_unrelated",
            "item_detail_unrelated",
            TurnItemLifecycleStatus::Completed,
        );

        manager.store.save_item(&first_late)?;
        manager.store.save_item(&second_item)?;
        manager.store.save_item(&unrelated)?;
        manager.store.save_item(&first_early)?;

        let detail = manager.get_thread_detail(&thread.id).await?;
        let item_ids: Vec<&str> = detail.items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            item_ids,
            vec![
                "item_detail_first_early",
                "item_detail_first_late",
                "item_detail_second"
            ]
        );
        Ok(())
    }

    #[tokio::test]
    async fn seed_thread_from_messages_keeps_parallel_tool_results_in_one_turn() -> Result<()> {
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
                ..Default::default()
            })
            .await?;

        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Calculate both files".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool_a".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path":"a.txt"}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tool_b".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path":"b.txt"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_a".to_string(),
                    content: "A".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_b".to_string(),
                    content: "B".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Done".to_string(),
                    cache_control: None,
                }],
            },
        ];

        manager
            .seed_thread_from_messages(&thread.id, &messages)
            .await?;

        let detail = manager.get_thread_detail(&thread.id).await?;
        assert_eq!(detail.turns.len(), 2);
        let merged_tool_result_turn_found =
            detail.turns.iter().try_fold(false, |found, turn| {
                let items = manager.store.list_items_for_turn(&turn.id)?;
                let tool_result_count = items
                    .iter()
                    .filter(|item| {
                        item.metadata
                            .as_ref()
                            .and_then(|meta| meta.get("tool_result_for"))
                            .is_some()
                    })
                    .count();
                let has_done = items.iter().any(|item| {
                    item.kind == TurnItemKind::AgentMessage
                        && item.detail.as_deref() == Some("Done")
                });
                Ok::<bool, anyhow::Error>(found || (tool_result_count == 2 && has_done))
            })?;
        assert!(merged_tool_result_turn_found);
        Ok(())
    }

    #[tokio::test]
    async fn reconstruct_messages_from_turns_batches_parallel_tool_results() -> Result<()> {
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
                ..Default::default()
            })
            .await?;

        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Calculate both files".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![
                    ContentBlock::ToolUse {
                        id: "tool_a".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path":"a.txt"}),
                        caller: None,
                    },
                    ContentBlock::ToolUse {
                        id: "tool_b".to_string(),
                        name: "read_file".to_string(),
                        input: serde_json::json!({"path":"b.txt"}),
                        caller: None,
                    },
                ],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_a".to_string(),
                    content: "A".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "tool_b".to_string(),
                    content: "B".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Text {
                    text: "Done".to_string(),
                    cache_control: None,
                }],
            },
        ];

        manager
            .seed_thread_from_messages(&thread.id, &messages)
            .await?;

        let turns = manager.store.list_turns_for_thread(&thread.id)?;
        let reconstructed = manager.reconstruct_messages_from_turns(&turns)?;
        let roles: Vec<&str> = reconstructed.iter().map(|msg| msg.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
        assert_eq!(reconstructed[2].content.len(), 2);
        assert!(
            reconstructed[2]
                .content
                .iter()
                .all(|block| matches!(block, ContentBlock::ToolResult { .. }))
        );
        Ok(())
    }

    #[tokio::test]
    async fn ensure_engine_loaded_uses_configured_sessions_dir() -> Result<()> {
        let data_dir = test_runtime_dir();
        let sessions_dir = data_dir.join("sessions-custom");
        fs::create_dir_all(&sessions_dir)?;
        let manager = test_manager_with_sessions_dir(data_dir, sessions_dir.clone())?;
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
                ..Default::default()
            })
            .await?;

        let messages = vec![
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::Text {
                    text: "restore me".to_string(),
                    cache_control: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::Thinking {
                    thinking: "hidden chain".to_string(),
                    signature: None,
                }],
            },
        ];
        let session = create_saved_session_with_id_and_mode(
            "sess_custom".to_string(),
            &messages,
            DEFAULT_TEXT_MODEL,
            Path::new("."),
            42,
            None,
            Some("agent"),
        );
        SessionManager::new(sessions_dir)?.save_session(&session)?;
        manager
            .set_thread_session_id(&thread.id, &session.metadata.id)
            .await?;

        let engine = manager.get_engine(&thread.id).await?;
        let snapshot = engine.get_session_snapshot().await?;
        assert_eq!(snapshot.messages.len(), 2);
        assert!(matches!(
            &snapshot.messages[1].content[0],
            ContentBlock::Thinking { thinking, .. } if thinking == "hidden chain"
        ));
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
                ..Default::default()
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
                    ..Default::default()
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
                ..Default::default()
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
                    ..Default::default()
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
                approval_grouping_key: "test_key".to_string(),
                id: "tool_stale".to_string(),
                tool_name: "exec_command".to_string(),
                description: "stale approval".to_string(),
                input: serde_json::json!({}),
                intent_summary: None,
                approval_force_prompt: false,
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
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
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
                    ..Default::default()
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
                approval_grouping_key: "key1".to_string(),
                id: "tool_external_allow".to_string(),
                tool_name: "exec_command".to_string(),
                description: "external allow".to_string(),
                input: serde_json::json!({}),
                intent_summary: Some("I will update the config file.".to_string()),
                approval_force_prompt: false,
            })
            .await?;

        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline && manager.pending_approvals_count() == 0 {
            sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(manager.pending_approvals_count(), 1);

        let events = manager.events_since(&thread.id, None)?;
        let approval_event = events
            .iter()
            .rev()
            .find(|event| event.event == "approval.required")
            .context("missing approval.required event")?;
        assert_eq!(
            approval_event
                .payload
                .get("intent_summary")
                .and_then(Value::as_str),
            Some("I will update the config file.")
        );

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
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
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
                    ..Default::default()
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
                approval_grouping_key: "key2".to_string(),
                id: "tool_external_deny".to_string(),
                tool_name: "exec_command".to_string(),
                description: "external deny".to_string(),
                input: serde_json::json!({}),
                intent_summary: None,
                approval_force_prompt: false,
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
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
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
                    ..Default::default()
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
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
            })
            .await?;
        assert!(!manager.store.load_thread(&thread.id)?.auto_approve);

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
                    auto_approve: None,
                    ..Default::default()
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
                approval_grouping_key: "key3".to_string(),
                id: "tool_remember".to_string(),
                tool_name: "exec_command".to_string(),
                description: "remember=true".to_string(),
                input: serde_json::json!({}),
                intent_summary: None,
                approval_force_prompt: false,
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
        assert_eq!(
            manager.active_turn_flags(&thread.id, &turn.id).await,
            Some((true, false)),
            "remember=true should update the active turn used by subsequent approvals"
        );

        harness
            .tx_event
            .send(EngineEvent::TurnComplete {
                usage: Usage::default(),
                status: TurnOutcomeStatus::Completed,
                error: None,
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
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
                    ..Default::default()
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
                tool_catalog: None,
                base_url: None,
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
                ..Default::default()
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
                        tool_catalog: None,
                        base_url: None,
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
                    ..Default::default()
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
                ..Default::default()
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
                                tool_catalog: None,
                                base_url: None,
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
                                tool_catalog: None,
                                base_url: None,
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
                    ..Default::default()
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
            session_id: None,
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
