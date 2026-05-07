use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use deepseek_agent::ModelRegistry;
use deepseek_config::{CliRuntimeOverrides, ConfigToml, ProviderKind};
use deepseek_execpolicy::{
    AskForApproval, ExecApprovalRequirement, ExecPolicyContext, ExecPolicyDecision,
    ExecPolicyEngine,
};
use deepseek_hooks::{HookDispatcher, HookEvent};
use deepseek_mcp::{
    McpManager, McpStartupCompleteEvent, McpStartupStatus as McpManagerStartupStatus,
};
use deepseek_protocol::{
    AppResponse, EventFrame, ExecApprovalRequestEvent, PromptRequest, PromptResponse,
    ReviewDecision, Thread, ThreadForkParams, ThreadListParams, ThreadReadParams, ThreadRequest,
    ThreadResponse, ThreadResumeParams, ThreadSetNameParams, ThreadStatus, ToolPayload,
};
use deepseek_state::{
    JobStateRecord, JobStateStatus, SessionSource, StateStore, ThreadListFilters, ThreadMetadata,
    ThreadStatus as PersistedThreadStatus,
};
use deepseek_tools::{ToolCall, ToolRegistry};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub enum InitialHistory {
    New,
    Forked(Vec<Value>),
    Resumed {
        conversation_id: String,
        history: Vec<Value>,
        rollout_path: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct NewThread {
    pub thread: Thread,
    pub model: String,
    pub model_provider: String,
    pub cwd: PathBuf,
    pub approval_policy: Option<String>,
    pub sandbox: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
}

const JOB_DETAIL_SCHEMA_VERSION: u8 = 1;
const DEFAULT_JOB_MAX_ATTEMPTS: u32 = 3;
const DEFAULT_JOB_BACKOFF_BASE_MS: u64 = 500;
const MAX_JOB_HISTORY_ENTRIES: usize = 64;

#[derive(Debug, Clone)]
pub struct JobRetryMetadata {
    pub attempt: u32,
    pub max_attempts: u32,
    pub backoff_base_ms: u64,
    pub next_backoff_ms: u64,
    pub next_retry_at: Option<i64>,
}

impl Default for JobRetryMetadata {
    fn default() -> Self {
        Self {
            attempt: 0,
            max_attempts: DEFAULT_JOB_MAX_ATTEMPTS,
            backoff_base_ms: DEFAULT_JOB_BACKOFF_BASE_MS,
            next_backoff_ms: 0,
            next_retry_at: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct JobHistoryEntry {
    pub at: i64,
    pub phase: String,
    pub status: JobStatus,
    pub progress: Option<u8>,
    pub detail: Option<String>,
    pub retry: JobRetryMetadata,
}

#[derive(Debug, Clone)]
struct PersistedJobDetail {
    pub status: JobStatus,
    pub detail: Option<String>,
    pub retry: JobRetryMetadata,
    pub history: Vec<JobHistoryEntry>,
}

#[derive(Debug, Clone)]
pub struct JobRecord {
    pub id: String,
    pub name: String,
    pub status: JobStatus,
    pub progress: Option<u8>,
    pub detail: Option<String>,
    pub retry: JobRetryMetadata,
    pub history: Vec<JobHistoryEntry>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Default)]
pub struct JobManager {
    jobs: HashMap<String, JobRecord>,
}

impl JobManager {
    fn now_ts() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn deterministic_backoff_ms(retry: &JobRetryMetadata) -> u64 {
        if retry.attempt == 0 {
            return 0;
        }
        let exponent = retry.attempt.saturating_sub(1).min(20);
        let multiplier = 1u64.checked_shl(exponent).unwrap_or(u64::MAX);
        retry.backoff_base_ms.saturating_mul(multiplier)
    }

    fn clear_retry_schedule(retry: &mut JobRetryMetadata) {
        retry.next_backoff_ms = 0;
        retry.next_retry_at = None;
    }

    fn push_history(job: &mut JobRecord, phase: &str) {
        job.history.push(JobHistoryEntry {
            at: job.updated_at,
            phase: phase.to_string(),
            status: job.status,
            progress: job.progress,
            detail: job.detail.clone(),
            retry: job.retry.clone(),
        });
        if job.history.len() > MAX_JOB_HISTORY_ENTRIES {
            let to_drain = job.history.len() - MAX_JOB_HISTORY_ENTRIES;
            job.history.drain(0..to_drain);
        }
    }

    fn parse_persisted_detail(raw: Option<&str>) -> Option<PersistedJobDetail> {
        let raw = raw?;
        let parsed: Value = serde_json::from_str(raw).ok()?;
        let status = parsed
            .get("status")
            .and_then(Value::as_str)
            .and_then(job_status_from_str)?;
        let detail = parsed.get("detail").and_then(json_optional_string);
        let retry = parse_retry_metadata(parsed.get("retry"));
        let history = parsed
            .get("history")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(parse_history_entry)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Some(PersistedJobDetail {
            status,
            detail,
            retry,
            history,
        })
    }

    fn encode_persisted_detail(job: &JobRecord) -> Result<Option<String>> {
        let encoded = json!({
            "schema_version": JOB_DETAIL_SCHEMA_VERSION,
            "status": job_status_to_str(job.status),
            "detail": job.detail.clone(),
            "retry": job_retry_to_value(&job.retry),
            "history": job.history.iter().map(job_history_to_value).collect::<Vec<_>>()
        })
        .to_string();
        Ok(Some(encoded))
    }

    pub fn enqueue(&mut self, name: impl Into<String>) -> JobRecord {
        let now = Self::now_ts();
        let id = format!("job-{}", Uuid::new_v4());
        let mut job = JobRecord {
            id: id.clone(),
            name: name.into(),
            status: JobStatus::Queued,
            progress: Some(0),
            detail: None,
            retry: JobRetryMetadata::default(),
            history: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        Self::push_history(&mut job, "created");
        self.jobs.insert(id, job.clone());
        job
    }

    pub fn set_running(&mut self, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Running;
            Self::clear_retry_schedule(&mut job.retry);
            job.updated_at = Self::now_ts();
            Self::push_history(job, "running");
        }
    }

    pub fn update_progress(&mut self, id: &str, progress: u8, detail: Option<String>) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.progress = Some(progress.min(100));
            job.detail = detail;
            job.updated_at = Self::now_ts();
            Self::push_history(job, "progress_updated");
        }
    }

    pub fn complete(&mut self, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Completed;
            job.progress = Some(100);
            Self::clear_retry_schedule(&mut job.retry);
            job.updated_at = Self::now_ts();
            Self::push_history(job, "completed");
        }
    }

    pub fn fail(&mut self, id: &str, detail: impl Into<String>) {
        if let Some(job) = self.jobs.get_mut(id) {
            let now = Self::now_ts();
            job.status = JobStatus::Failed;
            job.detail = Some(detail.into());
            if job.retry.attempt < job.retry.max_attempts {
                job.retry.attempt += 1;
                job.retry.next_backoff_ms = Self::deterministic_backoff_ms(&job.retry);
                let delay_secs = ((job.retry.next_backoff_ms.saturating_add(999)) / 1000)
                    .min(i64::MAX as u64) as i64;
                job.retry.next_retry_at = Some(now.saturating_add(delay_secs));
            } else {
                Self::clear_retry_schedule(&mut job.retry);
            }
            job.updated_at = now;
            Self::push_history(job, "failed");
        }
    }

    pub fn cancel(&mut self, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Cancelled;
            Self::clear_retry_schedule(&mut job.retry);
            job.updated_at = Self::now_ts();
            Self::push_history(job, "cancelled");
        }
    }

    pub fn pause(&mut self, id: &str, detail: Option<String>) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Paused;
            if detail.is_some() {
                job.detail = detail;
            }
            job.updated_at = Self::now_ts();
            Self::push_history(job, "paused");
        }
    }

    pub fn resume(&mut self, id: &str, detail: Option<String>) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.status = JobStatus::Running;
            if detail.is_some() {
                job.detail = detail;
            }
            Self::clear_retry_schedule(&mut job.retry);
            job.updated_at = Self::now_ts();
            Self::push_history(job, "resumed");
        }
    }

    pub fn list(&self) -> Vec<JobRecord> {
        let mut out = self.jobs.values().cloned().collect::<Vec<_>>();
        out.sort_by_key(|job| std::cmp::Reverse(job.updated_at));
        out
    }

    pub fn history(&self, id: &str) -> Vec<JobHistoryEntry> {
        self.jobs
            .get(id)
            .map(|job| job.history.clone())
            .unwrap_or_default()
    }

    pub fn resume_pending(&mut self) -> Vec<JobRecord> {
        let mut resumed = Vec::new();
        for job in self.jobs.values_mut() {
            if matches!(job.status, JobStatus::Queued | JobStatus::Running) {
                job.status = JobStatus::Queued;
                job.updated_at = Self::now_ts();
                Self::push_history(job, "queued_after_resume");
                resumed.push(job.clone());
            }
        }
        resumed
    }

    pub fn load_from_store(&mut self, store: &StateStore) -> Result<()> {
        let persisted = store.list_jobs(Some(500))?;
        for job in persisted {
            let fallback_status = job_state_status_to_runtime(job.status);
            let parsed = Self::parse_persisted_detail(job.detail.as_deref());
            let (status, detail, retry, history) = if let Some(detail_state) = parsed {
                (
                    detail_state.status,
                    detail_state.detail,
                    detail_state.retry,
                    detail_state.history,
                )
            } else {
                (
                    fallback_status,
                    job.detail,
                    JobRetryMetadata::default(),
                    Vec::new(),
                )
            };
            self.jobs.insert(
                job.id.clone(),
                JobRecord {
                    id: job.id,
                    name: job.name,
                    status,
                    progress: job.progress,
                    detail,
                    retry,
                    history,
                    created_at: job.created_at,
                    updated_at: job.updated_at,
                },
            );
        }
        Ok(())
    }

    pub fn persist_job(&self, store: &StateStore, id: &str) -> Result<()> {
        let Some(job) = self.jobs.get(id) else {
            return Ok(());
        };
        let encoded_detail = Self::encode_persisted_detail(job)?;
        store.upsert_job(&JobStateRecord {
            id: job.id.clone(),
            name: job.name.clone(),
            status: runtime_status_to_job_state(job.status),
            progress: job.progress,
            detail: encoded_detail,
            created_at: job.created_at,
            updated_at: job.updated_at,
        })
    }

    pub fn persist_all(&self, store: &StateStore) -> Result<()> {
        for id in self.jobs.keys() {
            self.persist_job(store, id)?;
        }
        Ok(())
    }
}

pub struct ThreadManager {
    store: StateStore,
    running_threads: HashMap<String, Thread>,
    cli_version: String,
}

impl ThreadManager {
    pub fn new(store: StateStore) -> Self {
        Self {
            store,
            running_threads: HashMap::new(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn state_store(&self) -> &StateStore {
        &self.store
    }

    pub fn spawn_thread_with_history(
        &mut self,
        model_provider: String,
        cwd: PathBuf,
        initial_history: InitialHistory,
        persist_extended_history: bool,
    ) -> Result<NewThread> {
        let id = format!("thread-{}", Uuid::new_v4());
        let now = chrono::Utc::now().timestamp();
        let preview = preview_from_initial_history(&initial_history);
        let source = match initial_history {
            InitialHistory::New => SessionSource::Interactive,
            InitialHistory::Forked(_) => SessionSource::Fork,
            InitialHistory::Resumed { .. } => SessionSource::Resume,
        };
        let thread = Thread {
            id: id.clone(),
            preview,
            ephemeral: !persist_extended_history,
            model_provider: model_provider.clone(),
            created_at: now,
            updated_at: now,
            status: ThreadStatus::Running,
            path: None,
            cwd: cwd.clone(),
            cli_version: self.cli_version.clone(),
            source: match source {
                SessionSource::Interactive => deepseek_protocol::SessionSource::Interactive,
                SessionSource::Resume => deepseek_protocol::SessionSource::Resume,
                SessionSource::Fork => deepseek_protocol::SessionSource::Fork,
                SessionSource::Api => deepseek_protocol::SessionSource::Api,
                SessionSource::Unknown => deepseek_protocol::SessionSource::Unknown,
            },
            name: None,
        };
        self.persist_thread(&thread, None)?;
        match &initial_history {
            InitialHistory::Forked(items) => {
                for item in items {
                    self.store.append_message(
                        &thread.id,
                        "history",
                        &item.to_string(),
                        Some(item.clone()),
                    )?;
                }
            }
            InitialHistory::Resumed { history, .. } => {
                for item in history {
                    self.store.append_message(
                        &thread.id,
                        "history",
                        &item.to_string(),
                        Some(item.clone()),
                    )?;
                }
            }
            InitialHistory::New => {}
        }
        self.running_threads
            .insert(thread.id.clone(), thread.clone());
        Ok(NewThread {
            thread,
            model: "auto".to_string(),
            model_provider,
            cwd,
            approval_policy: None,
            sandbox: None,
        })
    }

    pub fn resume_thread_with_history(
        &mut self,
        params: &ThreadResumeParams,
        fallback_cwd: &Path,
        model_provider: String,
    ) -> Result<Option<NewThread>> {
        if params.history.is_none()
            && let Some(thread) = self.running_threads.get(&params.thread_id).cloned()
        {
            return Ok(Some(NewThread {
                model: params.model.clone().unwrap_or_else(|| "auto".to_string()),
                model_provider: params.model_provider.clone().unwrap_or(model_provider),
                cwd: params.cwd.clone().unwrap_or_else(|| thread.cwd.clone()),
                approval_policy: params.approval_policy.clone(),
                sandbox: params.sandbox.clone(),
                thread,
            }));
        }

        let persisted = self.store.get_thread(&params.thread_id)?;
        let Some(metadata) = persisted else {
            return Ok(None);
        };
        let mut thread = to_protocol_thread(metadata);
        thread.status = ThreadStatus::Running;
        thread.updated_at = chrono::Utc::now().timestamp();
        thread.cwd = params
            .cwd
            .clone()
            .unwrap_or_else(|| fallback_cwd.to_path_buf());
        self.persist_thread(&thread, None)?;
        self.running_threads
            .insert(thread.id.clone(), thread.clone());
        if let Some(history) = params.history.as_ref() {
            for item in history {
                self.store.append_message(
                    &thread.id,
                    "history",
                    &item.to_string(),
                    Some(item.clone()),
                )?;
            }
        }

        Ok(Some(NewThread {
            model: params.model.clone().unwrap_or_else(|| "auto".to_string()),
            model_provider: params.model_provider.clone().unwrap_or(model_provider),
            cwd: thread.cwd.clone(),
            approval_policy: params.approval_policy.clone(),
            sandbox: params.sandbox.clone(),
            thread,
        }))
    }

    pub fn fork_thread(
        &mut self,
        params: &ThreadForkParams,
        fallback_cwd: &Path,
    ) -> Result<Option<NewThread>> {
        let parent = self.store.get_thread(&params.thread_id)?;
        let Some(parent) = parent else {
            return Ok(None);
        };
        let parent_thread = to_protocol_thread(parent);
        let new = self.spawn_thread_with_history(
            params
                .model_provider
                .clone()
                .unwrap_or_else(|| parent_thread.model_provider.clone()),
            params
                .cwd
                .clone()
                .unwrap_or_else(|| fallback_cwd.to_path_buf()),
            InitialHistory::Forked(vec![json!({
                "type": "fork",
                "from_thread_id": parent_thread.id
            })]),
            params.persist_extended_history,
        )?;
        Ok(Some(new))
    }

    pub fn list_threads(&self, params: &ThreadListParams) -> Result<Vec<Thread>> {
        let list = self.store.list_threads(ThreadListFilters {
            include_archived: params.include_archived,
            limit: params.limit,
        })?;
        Ok(list.into_iter().map(to_protocol_thread).collect())
    }

    pub fn read_thread(&self, params: &ThreadReadParams) -> Result<Option<Thread>> {
        Ok(self
            .store
            .get_thread(&params.thread_id)?
            .map(to_protocol_thread))
    }

    pub fn set_thread_name(&mut self, params: &ThreadSetNameParams) -> Result<Option<Thread>> {
        let Some(mut metadata) = self.store.get_thread(&params.thread_id)? else {
            return Ok(None);
        };
        metadata.name = Some(params.name.clone());
        metadata.updated_at = chrono::Utc::now().timestamp();
        self.store.upsert_thread(&metadata)?;
        let updated = to_protocol_thread(metadata);
        self.running_threads
            .insert(updated.id.clone(), updated.clone());
        Ok(Some(updated))
    }

    pub fn archive_thread(&mut self, thread_id: &str) -> Result<()> {
        self.store.mark_archived(thread_id)?;
        if let Some(thread) = self.running_threads.get_mut(thread_id) {
            thread.status = ThreadStatus::Archived;
        }
        Ok(())
    }

    pub fn unarchive_thread(&mut self, thread_id: &str) -> Result<()> {
        self.store.mark_unarchived(thread_id)?;
        Ok(())
    }

    pub fn touch_message(&mut self, thread_id: &str, input: &str) -> Result<()> {
        let Some(mut metadata) = self.store.get_thread(thread_id)? else {
            return Ok(());
        };
        metadata.updated_at = chrono::Utc::now().timestamp();
        metadata.preview = truncate_preview(input);
        metadata.status = PersistedThreadStatus::Running;
        self.store.upsert_thread(&metadata)?;
        if let Some(thread) = self.running_threads.get_mut(thread_id) {
            thread.updated_at = metadata.updated_at;
            thread.preview = metadata.preview;
            thread.status = ThreadStatus::Running;
        }
        let message_id = self.store.append_message(thread_id, "user", input, None)?;
        self.store.save_checkpoint(
            thread_id,
            "latest",
            &json!({
                "reason": "thread_message",
                "message_id": message_id,
                "role": "user",
                "preview": truncate_preview(input),
                "updated_at": metadata.updated_at
            }),
        )?;
        Ok(())
    }

    fn persist_thread(&self, thread: &Thread, rollout_path: Option<PathBuf>) -> Result<()> {
        self.store.upsert_thread(&ThreadMetadata {
            id: thread.id.clone(),
            rollout_path,
            preview: thread.preview.clone(),
            ephemeral: thread.ephemeral,
            model_provider: thread.model_provider.clone(),
            created_at: thread.created_at,
            updated_at: thread.updated_at,
            status: to_persisted_status(&thread.status),
            path: thread.path.clone(),
            cwd: thread.cwd.clone(),
            cli_version: thread.cli_version.clone(),
            source: to_persisted_source(&thread.source),
            name: thread.name.clone(),
            sandbox_policy: None,
            approval_mode: None,
            archived: matches!(thread.status, ThreadStatus::Archived),
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            memory_mode: None,
        })
    }
}

pub struct Runtime {
    pub config: ConfigToml,
    pub model_registry: ModelRegistry,
    pub thread_manager: ThreadManager,
    pub tool_registry: Arc<ToolRegistry>,
    pub mcp_manager: Arc<McpManager>,
    pub exec_policy: ExecPolicyEngine,
    pub hooks: HookDispatcher,
    pub jobs: JobManager,
}

impl Runtime {
    pub fn new(
        config: ConfigToml,
        model_registry: ModelRegistry,
        state: StateStore,
        tool_registry: Arc<ToolRegistry>,
        mcp_manager: Arc<McpManager>,
        exec_policy: ExecPolicyEngine,
        hooks: HookDispatcher,
    ) -> Self {
        let mut jobs = JobManager::default();
        let _ = jobs.load_from_store(&state);
        Self {
            config,
            model_registry,
            thread_manager: ThreadManager::new(state),
            tool_registry,
            mcp_manager,
            exec_policy,
            hooks,
            jobs,
        }
    }

    fn persisted_thread_data(&self, thread_id: &str) -> Result<Value> {
        let history = self
            .thread_manager
            .state_store()
            .list_messages(thread_id, Some(500))?
            .into_iter()
            .map(|message| {
                json!({
                    "id": message.id,
                    "role": message.role,
                    "content": message.content,
                    "item": message.item,
                    "created_at": message.created_at
                })
            })
            .collect::<Vec<_>>();

        let checkpoint = self
            .thread_manager
            .state_store()
            .load_checkpoint(thread_id, None)?
            .map(|record| {
                json!({
                    "checkpoint_id": record.checkpoint_id,
                    "state": record.state,
                    "created_at": record.created_at
                })
            });

        Ok(json!({
            "history": history,
            "checkpoint": checkpoint
        }))
    }

    fn persist_latest_checkpoint(&self, thread_id: &str, reason: &str, state: Value) -> Result<()> {
        self.thread_manager.state_store().save_checkpoint(
            thread_id,
            "latest",
            &json!({
                "reason": reason,
                "saved_at": chrono::Utc::now().timestamp(),
                "state": state
            }),
        )
    }

    pub async fn handle_thread(&mut self, req: ThreadRequest) -> Result<ThreadResponse> {
        match req {
            ThreadRequest::Create { .. } => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                let new = self.thread_manager.spawn_thread_with_history(
                    "deepseek".to_string(),
                    cwd,
                    InitialHistory::New,
                    false,
                )?;
                let mut response = thread_response_from_new("created", new);
                response.data = self.persisted_thread_data(&response.thread_id)?;
                Ok(response)
            }
            ThreadRequest::Start(params) => {
                let cwd = params.cwd.clone().unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
                let new = self.thread_manager.spawn_thread_with_history(
                    params
                        .model_provider
                        .clone()
                        .unwrap_or_else(|| "deepseek".to_string()),
                    cwd,
                    InitialHistory::New,
                    params.persist_extended_history,
                )?;
                let mut response = thread_response_from_new("started", new);
                response.data = self.persisted_thread_data(&response.thread_id)?;
                Ok(response)
            }
            ThreadRequest::Resume(params) => {
                let fallback_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if let Some(new) = self.thread_manager.resume_thread_with_history(
                    &params,
                    &fallback_cwd,
                    "deepseek".to_string(),
                )? {
                    let mut response = thread_response_from_new("resumed", new);
                    response.data = self.persisted_thread_data(&response.thread_id)?;
                    Ok(response)
                } else {
                    Ok(ThreadResponse {
                        thread_id: params.thread_id,
                        status: "missing".to_string(),
                        thread: None,
                        threads: Vec::new(),
                        model: None,
                        model_provider: None,
                        cwd: None,
                        approval_policy: params.approval_policy,
                        sandbox: params.sandbox,
                        events: Vec::new(),
                        data: json!({"error":"thread not found"}),
                    })
                }
            }
            ThreadRequest::Fork(params) => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                if let Some(new) = self.thread_manager.fork_thread(&params, &cwd)? {
                    let mut response = thread_response_from_new("forked", new);
                    response.data = self.persisted_thread_data(&response.thread_id)?;
                    Ok(response)
                } else {
                    Ok(ThreadResponse {
                        thread_id: params.thread_id,
                        status: "missing".to_string(),
                        thread: None,
                        threads: Vec::new(),
                        model: None,
                        model_provider: None,
                        cwd: None,
                        approval_policy: params.approval_policy,
                        sandbox: params.sandbox,
                        events: Vec::new(),
                        data: json!({"error":"thread not found"}),
                    })
                }
            }
            ThreadRequest::List(params) => Ok(ThreadResponse {
                thread_id: "list".to_string(),
                status: "ok".to_string(),
                thread: None,
                threads: self.thread_manager.list_threads(&params)?,
                model: None,
                model_provider: None,
                cwd: None,
                approval_policy: None,
                sandbox: None,
                events: Vec::new(),
                data: json!({}),
            }),
            ThreadRequest::Read(params) => {
                let id = params.thread_id.clone();
                let data = self.persisted_thread_data(&id)?;
                Ok(ThreadResponse {
                    thread_id: id,
                    status: "ok".to_string(),
                    thread: self.thread_manager.read_thread(&params)?,
                    threads: Vec::new(),
                    model: None,
                    model_provider: None,
                    cwd: None,
                    approval_policy: None,
                    sandbox: None,
                    events: Vec::new(),
                    data,
                })
            }
            ThreadRequest::SetName(params) => Ok(ThreadResponse {
                thread_id: params.thread_id.clone(),
                status: "ok".to_string(),
                thread: self.thread_manager.set_thread_name(&params)?,
                threads: Vec::new(),
                model: None,
                model_provider: None,
                cwd: None,
                approval_policy: None,
                sandbox: None,
                events: Vec::new(),
                data: json!({}),
            }),
            ThreadRequest::Archive { thread_id } => {
                self.thread_manager.archive_thread(&thread_id)?;
                Ok(ThreadResponse {
                    thread_id,
                    status: "archived".to_string(),
                    thread: None,
                    threads: Vec::new(),
                    model: None,
                    model_provider: None,
                    cwd: None,
                    approval_policy: None,
                    sandbox: None,
                    events: Vec::new(),
                    data: json!({}),
                })
            }
            ThreadRequest::Unarchive { thread_id } => {
                self.thread_manager.unarchive_thread(&thread_id)?;
                Ok(ThreadResponse {
                    thread_id,
                    status: "unarchived".to_string(),
                    thread: None,
                    threads: Vec::new(),
                    model: None,
                    model_provider: None,
                    cwd: None,
                    approval_policy: None,
                    sandbox: None,
                    events: Vec::new(),
                    data: json!({}),
                })
            }
            ThreadRequest::Message { thread_id, input } => {
                self.thread_manager.touch_message(&thread_id, &input)?;
                let response_id = format!("{thread_id}:{}", input.len());
                self.hooks
                    .emit(HookEvent::ResponseStart {
                        response_id: response_id.clone(),
                    })
                    .await;
                self.hooks
                    .emit(HookEvent::ResponseEnd {
                        response_id: response_id.clone(),
                    })
                    .await;

                Ok(ThreadResponse {
                    thread_id,
                    status: "accepted".to_string(),
                    thread: None,
                    threads: Vec::new(),
                    model: None,
                    model_provider: None,
                    cwd: None,
                    approval_policy: None,
                    sandbox: None,
                    events: vec![
                        EventFrame::ResponseStart {
                            response_id: response_id.clone(),
                        },
                        EventFrame::ResponseDelta {
                            response_id: response_id.clone(),
                            delta: "queued".to_string(),
                        },
                        EventFrame::ResponseEnd { response_id },
                    ],
                    data: json!({}),
                })
            }
        }
    }

    pub async fn handle_prompt(
        &mut self,
        req: PromptRequest,
        cli_overrides: &CliRuntimeOverrides,
    ) -> Result<PromptResponse> {
        let resolved = self.config.resolve_runtime_options(cli_overrides);
        let requested_model = req.model.clone().unwrap_or_else(|| resolved.model.clone());
        let selection = self
            .model_registry
            .resolve(Some(&requested_model), Some(resolved.provider));
        let resolved_model = selection.resolved.id.clone();
        let response_id = format!("resp-{}", Uuid::new_v4());

        self.hooks
            .emit(HookEvent::ResponseStart {
                response_id: response_id.clone(),
            })
            .await;
        self.hooks
            .emit(HookEvent::ResponseDelta {
                response_id: response_id.clone(),
                delta: "model-selected".to_string(),
            })
            .await;
        self.hooks
            .emit(HookEvent::ResponseEnd {
                response_id: response_id.clone(),
            })
            .await;

        let payload = json!({
            "provider": resolved.provider.as_str(),
            "model": resolved_model.clone(),
            "prompt": req.prompt,
            "telemetry": resolved.telemetry,
            "base_url": resolved.base_url,
            "has_api_key": resolved.api_key.as_ref().is_some_and(|k| !k.trim().is_empty()),
            "approval_policy": resolved.approval_policy,
            "sandbox_mode": resolved.sandbox_mode
        });
        if let Some(thread_id) = req.thread_id.as_ref() {
            self.thread_manager.touch_message(thread_id, &req.prompt)?;
            let assistant_message_id = self.thread_manager.store.append_message(
                thread_id,
                "assistant",
                &payload.to_string(),
                Some(payload.clone()),
            )?;
            self.persist_latest_checkpoint(
                thread_id,
                "prompt_response",
                json!({
                    "response_id": response_id.clone(),
                    "model": resolved_model.clone(),
                    "provider": resolved.provider.as_str(),
                    "assistant_message_id": assistant_message_id
                }),
            )?;
        }

        Ok(PromptResponse {
            output: payload.to_string(),
            model: resolved_model,
            events: vec![
                EventFrame::ResponseStart {
                    response_id: response_id.clone(),
                },
                EventFrame::ResponseDelta {
                    response_id: response_id.clone(),
                    delta: "model-selected".to_string(),
                },
                EventFrame::ResponseEnd { response_id },
            ],
        })
    }

    pub async fn invoke_tool(
        &self,
        call: ToolCall,
        approval_mode: AskForApproval,
        cwd: &Path,
    ) -> Result<Value> {
        let fallback_cwd = cwd.display().to_string();
        let (command, policy_cwd, execution_kind) = call.execution_subject(&fallback_cwd);
        let decision = self.exec_policy.check(ExecPolicyContext {
            command: &command,
            cwd: &policy_cwd,
            ask_for_approval: approval_mode,
            sandbox_mode: None,
        })?;
        let precheck = policy_precheck_payload(&decision, &command, &policy_cwd, execution_kind);
        let response_id = format!("tool-{}", Uuid::new_v4());
        let call_id = call
            .raw_tool_call_id
            .clone()
            .unwrap_or_else(|| format!("tool-call-{}", Uuid::new_v4()));
        self.hooks
            .emit(HookEvent::ToolLifecycle {
                response_id: response_id.clone(),
                tool_name: call.name.clone(),
                phase: "precheck".to_string(),
                payload: precheck.clone(),
            })
            .await;

        if !decision.allow {
            let reason = decision.reason().to_string();
            let approval_id = format!("approval-{}", Uuid::new_v4());
            let error_frame = EventFrame::Error {
                response_id: response_id.clone(),
                message: reason.clone(),
            };
            self.hooks
                .emit(HookEvent::ApprovalLifecycle {
                    approval_id,
                    phase: "denied".to_string(),
                    reason: Some(reason.clone()),
                })
                .await;
            self.hooks
                .emit(HookEvent::GenericEventFrame {
                    frame: error_frame.clone(),
                })
                .await;
            return Ok(json!({
                "ok": false,
                "status": "denied",
                "execution_kind": execution_kind,
                "response_id": response_id,
                "precheck": precheck,
                "error": reason,
                "events": [event_frame_payload(&error_frame)],
            }));
        }

        if decision.requires_approval {
            let approval_id = format!("approval-{}", Uuid::new_v4());
            let reason = decision.reason().to_string();
            let maybe_approval_frame = approval_request_frame(
                &decision.requirement,
                call_id,
                approval_id.clone(),
                response_id.clone(),
                command.clone(),
                policy_cwd.clone(),
            );
            self.hooks
                .emit(HookEvent::ApprovalLifecycle {
                    approval_id: approval_id.clone(),
                    phase: "requested".to_string(),
                    reason: Some(reason.clone()),
                })
                .await;
            let mut events = Vec::new();
            if let Some(frame) = maybe_approval_frame {
                self.hooks
                    .emit(HookEvent::GenericEventFrame {
                        frame: frame.clone(),
                    })
                    .await;
                events.push(event_frame_payload(&frame));
            }
            return Ok(json!({
                "ok": false,
                "status": "approval_required",
                "execution_kind": execution_kind,
                "response_id": response_id,
                "approval_id": approval_id,
                "precheck": precheck,
                "error": reason,
                "events": events,
            }));
        }

        let start_frame = EventFrame::ToolCallStart {
            response_id: response_id.clone(),
            tool_name: call.name.clone(),
            arguments: tool_payload_value(&call.payload),
        };
        self.hooks
            .emit(HookEvent::GenericEventFrame {
                frame: start_frame.clone(),
            })
            .await;
        self.hooks
            .emit(HookEvent::ToolLifecycle {
                response_id: response_id.clone(),
                tool_name: call.name.clone(),
                phase: "dispatching".to_string(),
                payload: json!({
                    "call_id": call_id,
                    "execution_kind": execution_kind
                }),
            })
            .await;

        match self.tool_registry.dispatch(call.clone(), true).await {
            Ok(tool_output) => {
                let result_frame = EventFrame::ToolCallResult {
                    response_id: response_id.clone(),
                    tool_name: call.name.clone(),
                    output: tool_output_value(&tool_output),
                };
                self.hooks
                    .emit(HookEvent::GenericEventFrame {
                        frame: result_frame.clone(),
                    })
                    .await;
                self.hooks
                    .emit(HookEvent::ToolLifecycle {
                        response_id: response_id.clone(),
                        tool_name: call.name,
                        phase: "completed".to_string(),
                        payload: json!({ "ok": true }),
                    })
                    .await;
                Ok(json!({
                    "ok": true,
                    "status": "completed",
                    "execution_kind": execution_kind,
                    "response_id": response_id,
                    "precheck": precheck,
                    "output": tool_output,
                    "events": [
                        event_frame_payload(&start_frame),
                        event_frame_payload(&result_frame)
                    ]
                }))
            }
            Err(err) => {
                let message = format!("{err:?}");
                let error_frame = EventFrame::Error {
                    response_id: response_id.clone(),
                    message: message.clone(),
                };
                self.hooks
                    .emit(HookEvent::GenericEventFrame {
                        frame: error_frame.clone(),
                    })
                    .await;
                self.hooks
                    .emit(HookEvent::ToolLifecycle {
                        response_id: response_id.clone(),
                        tool_name: call.name,
                        phase: "failed".to_string(),
                        payload: json!({ "error": message.clone() }),
                    })
                    .await;
                Ok(json!({
                    "ok": false,
                    "status": "failed",
                    "execution_kind": execution_kind,
                    "response_id": response_id,
                    "precheck": precheck,
                    "error": message,
                    "events": [
                        event_frame_payload(&start_frame),
                        event_frame_payload(&error_frame)
                    ]
                }))
            }
        }
    }

    pub async fn mcp_startup(&self) -> McpStartupCompleteEvent {
        let mut updates = Vec::new();
        let summary = self.mcp_manager.start_all(|update| {
            updates.push(update);
        });
        for update in updates {
            let status = match update.status {
                McpManagerStartupStatus::Starting => deepseek_protocol::McpStartupStatus::Starting,
                McpManagerStartupStatus::Ready => deepseek_protocol::McpStartupStatus::Ready,
                McpManagerStartupStatus::Failed { error } => {
                    deepseek_protocol::McpStartupStatus::Failed { error }
                }
                McpManagerStartupStatus::Cancelled => {
                    deepseek_protocol::McpStartupStatus::Cancelled
                }
            };
            self.hooks
                .emit(HookEvent::GenericEventFrame {
                    frame: EventFrame::McpStartupUpdate {
                        update: deepseek_protocol::McpStartupUpdateEvent {
                            server_name: update.server_name,
                            status,
                        },
                    },
                })
                .await;
        }
        self.hooks
            .emit(HookEvent::GenericEventFrame {
                frame: EventFrame::McpStartupComplete {
                    summary: deepseek_protocol::McpStartupCompleteEvent {
                        ready: summary.ready.clone(),
                        failed: summary
                            .failed
                            .iter()
                            .map(|f| deepseek_protocol::McpStartupFailure {
                                server_name: f.server_name.clone(),
                                error: f.error.clone(),
                            })
                            .collect(),
                        cancelled: summary.cancelled.clone(),
                    },
                },
            })
            .await;
        summary
    }

    pub fn app_status(&self) -> AppResponse {
        let jobs = self.jobs.list();
        let events = jobs
            .iter()
            .flat_map(|job| {
                job.history.iter().map(|entry| EventFrame::ResponseDelta {
                    response_id: job.id.clone(),
                    delta: json!({
                        "kind": "job_transition",
                        "job_id": job.id.clone(),
                        "phase": entry.phase.clone(),
                        "status": job_status_to_str(entry.status),
                        "progress": entry.progress,
                        "detail": entry.detail.clone(),
                        "retry": job_retry_to_value(&entry.retry),
                        "at": entry.at
                    })
                    .to_string(),
                })
            })
            .collect::<Vec<_>>();
        AppResponse {
            ok: true,
            data: json!({
                "jobs": jobs.into_iter().map(|job| {
                    json!({
                        "id": job.id,
                        "name": job.name,
                        "status": job_status_to_str(job.status),
                        "progress": job.progress,
                        "detail": job.detail,
                        "retry": job_retry_to_value(&job.retry),
                        "history": job.history.iter().map(job_history_to_value).collect::<Vec<_>>()
                    })
                }).collect::<Vec<_>>()
            }),
            events,
        }
    }

    pub fn provider_default(&self) -> ProviderKind {
        self.config.provider
    }

    pub fn save_thread_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: &str,
        state: &Value,
    ) -> Result<()> {
        self.thread_manager
            .state_store()
            .save_checkpoint(thread_id, checkpoint_id, state)
    }

    pub fn load_thread_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: Option<&str>,
    ) -> Result<Option<Value>> {
        Ok(self
            .thread_manager
            .state_store()
            .load_checkpoint(thread_id, checkpoint_id)?
            .map(|checkpoint| checkpoint.state))
    }

    pub fn enqueue_job(&mut self, name: impl Into<String>) -> Result<JobRecord> {
        let job = self.jobs.enqueue(name);
        self.jobs
            .persist_job(self.thread_manager.state_store(), &job.id)?;
        Ok(job)
    }

    pub fn set_job_running(&mut self, job_id: &str) -> Result<()> {
        self.jobs.set_running(job_id);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn update_job_progress(
        &mut self,
        job_id: &str,
        progress: u8,
        detail: Option<String>,
    ) -> Result<()> {
        self.jobs.update_progress(job_id, progress, detail);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn complete_job(&mut self, job_id: &str) -> Result<()> {
        self.jobs.complete(job_id);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn fail_job(&mut self, job_id: &str, detail: impl Into<String>) -> Result<()> {
        self.jobs.fail(job_id, detail);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn cancel_job(&mut self, job_id: &str) -> Result<()> {
        self.jobs.cancel(job_id);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn pause_job(&mut self, job_id: &str, detail: Option<String>) -> Result<()> {
        self.jobs.pause(job_id, detail);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn resume_job(&mut self, job_id: &str, detail: Option<String>) -> Result<()> {
        self.jobs.resume(job_id, detail);
        self.jobs
            .persist_job(self.thread_manager.state_store(), job_id)
    }

    pub fn job_history(&self, job_id: &str) -> Vec<JobHistoryEntry> {
        self.jobs.history(job_id)
    }
}

fn thread_response_from_new(status: &str, new: NewThread) -> ThreadResponse {
    ThreadResponse {
        thread_id: new.thread.id.clone(),
        status: status.to_string(),
        thread: Some(new.thread),
        threads: Vec::new(),
        model: Some(new.model),
        model_provider: Some(new.model_provider),
        cwd: Some(new.cwd),
        approval_policy: new.approval_policy,
        sandbox: new.sandbox,
        events: Vec::new(),
        data: json!({}),
    }
}

fn preview_from_initial_history(initial_history: &InitialHistory) -> String {
    match initial_history {
        InitialHistory::New => "New conversation".to_string(),
        InitialHistory::Forked(items) => truncate_preview(
            &items
                .first()
                .map(Value::to_string)
                .unwrap_or_else(|| "Forked conversation".to_string()),
        ),
        InitialHistory::Resumed { history, .. } => truncate_preview(
            &history
                .first()
                .map(Value::to_string)
                .unwrap_or_else(|| "Resumed conversation".to_string()),
        ),
    }
}

fn truncate_preview(value: &str) -> String {
    value.chars().take(120).collect()
}

fn to_protocol_thread(thread: ThreadMetadata) -> Thread {
    Thread {
        id: thread.id,
        preview: thread.preview,
        ephemeral: thread.ephemeral,
        model_provider: thread.model_provider,
        created_at: thread.created_at,
        updated_at: thread.updated_at,
        status: match thread.status {
            PersistedThreadStatus::Running => ThreadStatus::Running,
            PersistedThreadStatus::Idle => ThreadStatus::Idle,
            PersistedThreadStatus::Completed => ThreadStatus::Completed,
            PersistedThreadStatus::Failed => ThreadStatus::Failed,
            PersistedThreadStatus::Paused => ThreadStatus::Paused,
            PersistedThreadStatus::Archived => ThreadStatus::Archived,
        },
        path: thread.path,
        cwd: thread.cwd,
        cli_version: thread.cli_version,
        source: match thread.source {
            SessionSource::Interactive => deepseek_protocol::SessionSource::Interactive,
            SessionSource::Resume => deepseek_protocol::SessionSource::Resume,
            SessionSource::Fork => deepseek_protocol::SessionSource::Fork,
            SessionSource::Api => deepseek_protocol::SessionSource::Api,
            SessionSource::Unknown => deepseek_protocol::SessionSource::Unknown,
        },
        name: thread.name,
    }
}

fn to_persisted_status(status: &ThreadStatus) -> PersistedThreadStatus {
    match status {
        ThreadStatus::Running => PersistedThreadStatus::Running,
        ThreadStatus::Idle => PersistedThreadStatus::Idle,
        ThreadStatus::Completed => PersistedThreadStatus::Completed,
        ThreadStatus::Failed => PersistedThreadStatus::Failed,
        ThreadStatus::Paused => PersistedThreadStatus::Paused,
        ThreadStatus::Archived => PersistedThreadStatus::Archived,
    }
}

fn to_persisted_source(source: &deepseek_protocol::SessionSource) -> SessionSource {
    match source {
        deepseek_protocol::SessionSource::Interactive => SessionSource::Interactive,
        deepseek_protocol::SessionSource::Resume => SessionSource::Resume,
        deepseek_protocol::SessionSource::Fork => SessionSource::Fork,
        deepseek_protocol::SessionSource::Api => SessionSource::Api,
        deepseek_protocol::SessionSource::Unknown => SessionSource::Unknown,
    }
}

fn approval_request_frame(
    requirement: &ExecApprovalRequirement,
    call_id: String,
    approval_id: String,
    turn_id: String,
    command: String,
    cwd: String,
) -> Option<EventFrame> {
    let ExecApprovalRequirement::NeedsApproval {
        reason,
        proposed_execpolicy_amendment,
        proposed_network_policy_amendments,
    } = requirement
    else {
        return None;
    };

    let mut available_decisions = vec![
        ReviewDecision::Approved,
        ReviewDecision::ApprovedForSession,
        ReviewDecision::Denied,
        ReviewDecision::Abort,
    ];
    if proposed_execpolicy_amendment
        .as_ref()
        .is_some_and(|amendment| !amendment.prefixes.is_empty())
    {
        available_decisions.push(ReviewDecision::ApprovedExecpolicyAmendment);
    }
    available_decisions.extend(proposed_network_policy_amendments.iter().cloned().map(
        |amendment| ReviewDecision::NetworkPolicyAmendment {
            host: amendment.host,
            action: amendment.action,
        },
    ));

    Some(EventFrame::ExecApprovalRequest {
        request: ExecApprovalRequestEvent {
            call_id,
            approval_id,
            turn_id,
            command,
            cwd,
            reason: reason.clone(),
            network_approval_context: None,
            proposed_execpolicy_amendment: proposed_execpolicy_amendment
                .as_ref()
                .map(|amendment| amendment.prefixes.clone())
                .unwrap_or_default(),
            proposed_network_policy_amendments: proposed_network_policy_amendments.clone(),
            additional_permissions: Vec::new(),
            available_decisions,
        },
    })
}

fn approval_requirement_payload(requirement: &ExecApprovalRequirement) -> Value {
    match requirement {
        ExecApprovalRequirement::Skip {
            bypass_sandbox,
            proposed_execpolicy_amendment,
        } => json!({
            "type": "skip",
            "bypass_sandbox": bypass_sandbox,
            "reason": requirement.reason(),
            "proposed_execpolicy_amendment": proposed_execpolicy_amendment
                .as_ref()
                .map(|amendment| amendment.prefixes.clone())
                .unwrap_or_default()
        }),
        ExecApprovalRequirement::NeedsApproval {
            reason,
            proposed_execpolicy_amendment,
            proposed_network_policy_amendments,
        } => json!({
            "type": "needs_approval",
            "reason": reason,
            "proposed_execpolicy_amendment": proposed_execpolicy_amendment
                .as_ref()
                .map(|amendment| amendment.prefixes.clone())
                .unwrap_or_default(),
            "proposed_network_policy_amendments": proposed_network_policy_amendments
        }),
        ExecApprovalRequirement::Forbidden { reason } => json!({
            "type": "forbidden",
            "reason": reason
        }),
    }
}

fn policy_precheck_payload(
    decision: &ExecPolicyDecision,
    command: &str,
    cwd: &str,
    execution_kind: &str,
) -> Value {
    json!({
        "execution_kind": execution_kind,
        "command": command,
        "cwd": cwd,
        "allow": decision.allow,
        "requires_approval": decision.requires_approval,
        "matched_rule": decision.matched_rule.clone(),
        "phase": decision.requirement.phase(),
        "reason": decision.reason(),
        "requirement": approval_requirement_payload(&decision.requirement)
    })
}

fn tool_payload_value(payload: &ToolPayload) -> Value {
    serde_json::to_value(payload).unwrap_or_else(
        |_| json!({"type":"serialization_error","message":"tool payload unavailable"}),
    )
}

fn tool_output_value(output: &deepseek_protocol::ToolOutput) -> Value {
    serde_json::to_value(output).unwrap_or_else(
        |_| json!({"type":"serialization_error","message":"tool output unavailable"}),
    )
}

fn event_frame_payload(frame: &EventFrame) -> Value {
    serde_json::to_value(frame)
        .unwrap_or_else(|_| json!({"event":"error","message":"failed to encode event frame"}))
}

fn json_optional_string(value: &Value) -> Option<String> {
    if value.is_null() {
        None
    } else {
        value.as_str().map(ToString::to_string)
    }
}

fn parse_retry_metadata(value: Option<&Value>) -> JobRetryMetadata {
    let Some(value) = value else {
        return JobRetryMetadata::default();
    };
    JobRetryMetadata {
        attempt: value
            .get("attempt")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
        max_attempts: value
            .get("max_attempts")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_JOB_MAX_ATTEMPTS as u64)
            .min(u32::MAX as u64) as u32,
        backoff_base_ms: value
            .get("backoff_base_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_JOB_BACKOFF_BASE_MS),
        next_backoff_ms: value
            .get("next_backoff_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        next_retry_at: value.get("next_retry_at").and_then(Value::as_i64),
    }
}

fn parse_history_entry(value: &Value) -> Option<JobHistoryEntry> {
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .and_then(job_status_from_str)?;
    Some(JobHistoryEntry {
        at: value.get("at").and_then(Value::as_i64).unwrap_or(0),
        phase: value
            .get("phase")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        status,
        progress: value
            .get("progress")
            .and_then(Value::as_u64)
            .map(|v| v.min(u8::MAX as u64) as u8),
        detail: value.get("detail").and_then(json_optional_string),
        retry: parse_retry_metadata(value.get("retry")),
    })
}

fn job_status_to_str(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Queued => "queued",
        JobStatus::Running => "running",
        JobStatus::Paused => "paused",
        JobStatus::Completed => "completed",
        JobStatus::Failed => "failed",
        JobStatus::Cancelled => "cancelled",
    }
}

fn job_status_from_str(value: &str) -> Option<JobStatus> {
    match value {
        "queued" => Some(JobStatus::Queued),
        "running" => Some(JobStatus::Running),
        "paused" => Some(JobStatus::Paused),
        "completed" => Some(JobStatus::Completed),
        "failed" => Some(JobStatus::Failed),
        "cancelled" => Some(JobStatus::Cancelled),
        _ => None,
    }
}

fn job_retry_to_value(retry: &JobRetryMetadata) -> Value {
    json!({
        "attempt": retry.attempt,
        "max_attempts": retry.max_attempts,
        "backoff_base_ms": retry.backoff_base_ms,
        "next_backoff_ms": retry.next_backoff_ms,
        "next_retry_at": retry.next_retry_at
    })
}

fn job_history_to_value(entry: &JobHistoryEntry) -> Value {
    json!({
        "at": entry.at,
        "phase": entry.phase.clone(),
        "status": job_status_to_str(entry.status),
        "progress": entry.progress,
        "detail": entry.detail.clone(),
        "retry": job_retry_to_value(&entry.retry)
    })
}

fn runtime_status_to_job_state(status: JobStatus) -> JobStateStatus {
    match status {
        JobStatus::Queued => JobStateStatus::Queued,
        JobStatus::Running => JobStateStatus::Running,
        JobStatus::Paused => JobStateStatus::Running,
        JobStatus::Completed => JobStateStatus::Completed,
        JobStatus::Failed => JobStateStatus::Failed,
        JobStatus::Cancelled => JobStateStatus::Cancelled,
    }
}

fn job_state_status_to_runtime(status: JobStateStatus) -> JobStatus {
    match status {
        JobStateStatus::Queued => JobStatus::Queued,
        JobStateStatus::Running => JobStatus::Running,
        JobStateStatus::Completed => JobStatus::Completed,
        JobStateStatus::Failed => JobStatus::Failed,
        JobStateStatus::Cancelled => JobStatus::Cancelled,
    }
}
