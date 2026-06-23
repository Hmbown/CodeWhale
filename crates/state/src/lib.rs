//! Persistent state management for conversation threads, messages, and jobs.
//!
//! The [`StateStore`] is the primary entry point, backed by a SQLite database and an
//! append-only JSONL session index file. It provides CRUD operations for:
//!
//! - **Threads** — conversation metadata, archival, and session indexing.
//! - **Messages** — append-only message storage with tree-structured branching.
//! - **Checkpoints** — named state snapshots for restoring conversation progress.
//! - **Jobs** — background task tracking with status and progress.
//! - **Dynamic tools** — per-thread tool registrations.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Lifecycle status of a conversation thread.
///
/// Serialized as lowercase snake_case strings (e.g. `"running"`, `"archived"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    /// Thread is actively being worked on.
    Running,
    /// Thread exists but has no active work in progress.
    Idle,
    /// Thread has finished its task successfully.
    Completed,
    /// Thread encountered an unrecoverable error.
    Failed,
    /// Thread has been temporarily paused by the user.
    Paused,
    /// Thread has been archived and is hidden from default listings.
    Archived,
}

/// Indicates how a session was initiated.
///
/// Serialized as lowercase snake_case strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    /// Started by a user interacting with the CLI.
    Interactive,
    /// Resumed from a previously persisted session.
    Resume,
    /// Created by forking an existing conversation at a specific message.
    Fork,
    /// Initiated programmatically via the API.
    Api,
    /// Source is unknown or unspecified.
    Unknown,
}

/// Metadata for a persisted conversation thread.
///
/// Each thread represents a single conversation session and stores its
/// configuration, git context, and current status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadMetadata {
    /// Unique identifier for this thread.
    pub id: String,
    /// Optional filesystem path to the rollout (JSONL transcript) file.
    pub rollout_path: Option<PathBuf>,
    /// Short preview or summary of the thread content.
    pub preview: String,
    /// Whether this thread is ephemeral (not persisted long-term).
    pub ephemeral: bool,
    /// Identifier of the model provider used for this thread (e.g. `"openai"`).
    pub model_provider: String,
    /// Unix timestamp (seconds) when the thread was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) of the most recent update to the thread.
    pub updated_at: i64,
    /// Current lifecycle status of the thread.
    pub status: ThreadStatus,
    /// Optional filesystem path associated with the thread working context.
    pub path: Option<PathBuf>,
    /// Working directory that was active when the thread was created.
    pub cwd: PathBuf,
    /// Version of the CLI that created this thread.
    pub cli_version: String,
    /// How this session was initiated.
    pub source: SessionSource,
    /// User-assigned display name for the thread.
    pub name: Option<String>,
    /// Serialized sandbox policy applied to this thread, if any.
    pub sandbox_policy: Option<String>,
    /// Approval mode configured for tool calls in this thread.
    pub approval_mode: Option<String>,
    /// Whether the thread has been archived.
    pub archived: bool,
    /// Unix timestamp (seconds) when the thread was archived, or `None` if not archived.
    pub archived_at: Option<i64>,
    /// Git commit SHA of the working tree when the thread was created.
    pub git_sha: Option<String>,
    /// Git branch checked out when the thread was created.
    pub git_branch: Option<String>,
    /// URL of the git remote origin, if available.
    pub git_origin_url: Option<String>,
    /// Memory mode configured for this thread (e.g. `"local"`, `"remote"`).
    pub memory_mode: Option<String>,
    /// ID of the current leaf message in the conversation tree.
    pub current_leaf_id: Option<i64>,
}

/// A dynamically registered tool associated with a thread.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicToolRecord {
    /// Ordinal position of this tool in the thread tool list.
    pub position: i64,
    /// Unique name identifying the tool.
    pub name: String,
    /// Human-readable description of what the tool does.
    pub description: Option<String>,
    /// JSON Schema describing the tool input parameters.
    pub input_schema: Value,
}

/// A single message entry in a conversation thread.
///
/// Messages form a tree structure via [`parent_entry_id`](Self::parent_entry_id),
/// enabling conversation branching and forking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// Auto-incremented unique identifier for this message.
    pub id: i64,
    /// ID of the thread this message belongs to.
    pub thread_id: String,
    /// Role of the message sender (e.g. `"user"`, `"assistant"`, `"system"`).
    pub role: String,
    /// Text content of the message.
    pub content: String,
    /// Optional structured item payload (tool calls, tool results, etc.).
    pub item: Option<Value>,
    /// Unix timestamp (seconds) when the message was created.
    pub created_at: i64,
    /// ID of the parent message, forming a tree structure. `None` for root messages.
    pub parent_entry_id: Option<i64>,
}

/// A named checkpoint capturing the state of a thread at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointRecord {
    /// ID of the thread this checkpoint belongs to.
    pub thread_id: String,
    /// Unique identifier for this checkpoint within its thread.
    pub checkpoint_id: String,
    /// Serialized state snapshot stored as a JSON value.
    pub state: Value,
    /// Unix timestamp (seconds) when the checkpoint was created or last updated.
    pub created_at: i64,
}

/// Status of a background job.
///
/// Serialized as lowercase snake_case strings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JobStateStatus {
    /// Job is waiting to be executed.
    Queued,
    /// Job is currently executing.
    Running,
    /// Job has finished successfully.
    Completed,
    /// Job has failed with an error.
    Failed,
    /// Job was cancelled before completion.
    Cancelled,
}

/// Persisted state of a background job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobStateRecord {
    /// Unique identifier for the job.
    pub id: String,
    /// Human-readable name describing the job.
    pub name: String,
    /// Current lifecycle status of the job.
    pub status: JobStateStatus,
    /// Completion progress as a percentage (0--100), if available.
    pub progress: Option<u8>,
    /// Optional detail message providing additional status information.
    pub detail: Option<String>,
    /// Unix timestamp (seconds) when the job was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) of the most recent status update.
    pub updated_at: i64,
}

/// Persisted lifecycle status for a thread goal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThreadGoalStatus {
    /// Goal is active and should continue receiving work.
    Active,
    /// Goal is paused by the user.
    Paused,
    /// Goal is blocked and cannot make meaningful progress.
    Blocked,
    /// Goal stopped because account/service usage limits were reached.
    UsageLimited,
    /// Goal stopped because its explicit token budget was reached.
    BudgetLimited,
    /// Goal has been completed.
    Complete,
}

/// Persisted goal state attached to a thread.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadGoalRecord {
    /// Thread this goal belongs to.
    pub thread_id: String,
    /// Stable identifier for this goal revision.
    pub goal_id: String,
    /// User-visible objective.
    pub objective: String,
    /// Current lifecycle status.
    pub status: ThreadGoalStatus,
    /// Optional token budget requested by the user.
    pub token_budget: Option<i64>,
    /// Tokens consumed while pursuing the goal.
    pub tokens_used: i64,
    /// Elapsed wall-clock work time in seconds.
    pub time_used_seconds: i64,
    /// Durable continuation passes dispatched for this objective.
    pub continuation_count: i64,
    /// Unix timestamp (seconds) when the goal was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) when the goal was last updated.
    pub updated_at: i64,
}

/// Filters for listing conversation threads.
#[derive(Debug, Clone)]
pub struct ThreadListFilters {
    /// Whether to include archived threads in the results.
    pub include_archived: bool,
    /// Maximum number of threads to return. Defaults to 50.
    pub limit: Option<usize>,
}

impl Default for ThreadListFilters {
    fn default() -> Self {
        Self {
            include_archived: false,
            limit: Some(50),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionIndexEntry {
    thread_id: String,
    thread_name: Option<String>,
    updated_at: i64,
    rollout_path: Option<PathBuf>,
}

/// Persistent storage for conversation threads, messages, checkpoints, and jobs.
///
/// Backed by a SQLite database and an append-only JSONL session index file.
/// The database schema is automatically initialized and migrated on [`open`](Self::open).
#[derive(Debug, Clone)]
pub struct StateStore {
    db_path: PathBuf,
    session_index_path: PathBuf,
}

impl StateStore {
    /// Open (or create) a state store at the given database path.
    ///
    /// If `path` is `None`, the default location (`~/.codewhale/state.db`, with
    /// `~/.deepseek/state.db` as a legacy fallback) is used.
    /// The database schema is created automatically if it does not exist.
    pub fn open(path: Option<PathBuf>) -> Result<Self> {
        let db_path = path.unwrap_or_else(default_state_db_path);
        let session_index_path = db_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("session_index.jsonl");
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create state directory {}", parent.display())
            })?;
        }
        let store = Self {
            db_path,
            session_index_path,
        };
        store.init_schema()?;
        Ok(store)
    }

    /// Returns the filesystem path of the underlying SQLite database.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn conn(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("failed to open state db {}", self.db_path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")
            .context("failed to enable sqlite foreign keys")?;
        Ok(conn)
    }

    fn init_schema(&self) -> Result<()> {
        let conn = self.conn()?;
        let mut user_version: u32 = conn.query_row("PRAGMA user_version;", [], |row| row.get(0))?;
        if user_version == 0 {
            conn.execute_batch(
                r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS threads (
                    id TEXT PRIMARY KEY,
                    rollout_path TEXT,
                    preview TEXT NOT NULL,
                    ephemeral INTEGER NOT NULL,
                    model_provider TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    status TEXT NOT NULL,
                    path TEXT,
                    cwd TEXT NOT NULL,
                    cli_version TEXT NOT NULL,
                    source TEXT NOT NULL,
                    title TEXT,
                    sandbox_policy TEXT,
                    approval_mode TEXT,
                    archived INTEGER NOT NULL DEFAULT 0,
                    archived_at INTEGER,
                    git_sha TEXT,
                    git_branch TEXT,
                    git_origin_url TEXT,
                    memory_mode TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_threads_updated_at ON threads(updated_at DESC);
                CREATE INDEX IF NOT EXISTS idx_threads_archived_at ON threads(archived_at DESC);
                CREATE INDEX IF NOT EXISTS idx_threads_archived_updated ON threads(archived, updated_at DESC);

                CREATE TABLE IF NOT EXISTS thread_dynamic_tools (
                    thread_id TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    name TEXT NOT NULL,
                    description TEXT,
                    input_schema TEXT NOT NULL,
                    PRIMARY KEY (thread_id, position),
                    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    thread_id TEXT NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT NOT NULL,
                    item_json TEXT,
                    created_at INTEGER NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_messages_thread_created_at ON messages(thread_id, created_at ASC);

                CREATE TABLE IF NOT EXISTS checkpoints (
                    thread_id TEXT NOT NULL,
                    checkpoint_id TEXT NOT NULL,
                    state_json TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    PRIMARY KEY(thread_id, checkpoint_id),
                    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_checkpoints_thread_created_at ON checkpoints(thread_id, created_at DESC);

                CREATE TABLE IF NOT EXISTS jobs (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    status TEXT NOT NULL,
                    progress INTEGER,
                    detail TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_jobs_updated_at ON jobs(updated_at DESC);

                -- Add parent_entry_id column, and set to last message before current message
                ALTER TABLE messages ADD COLUMN parent_entry_id INTEGER NULL;
                UPDATE messages
                    SET parent_entry_id = (
                        SELECT m2.id
                        FROM messages m2
                        WHERE m2.thread_id = messages.thread_id
                            AND (
                                m2.created_at < messages.created_at
                                OR (
                                    m2.created_at = messages.created_at
                                    AND m2.id < messages.id
                                )
                            )
                        ORDER BY m2.created_at DESC, m2.id DESC
                        LIMIT 1
                    );
                CREATE INDEX idx_messages_parent_entry_id ON messages(parent_entry_id);

                -- Add current_leaf_id column, and set to last message in thread
                ALTER TABLE threads ADD COLUMN current_leaf_id INTEGER NULL;
                UPDATE threads
                    SET current_leaf_id = (
                        SELECT m.id
                        FROM messages m
                        WHERE m.thread_id = threads.id
                        ORDER BY m.id DESC
                        LIMIT 1
                    );

                PRAGMA user_version = 1;
                COMMIT;
                "#,
            )
            .context("failed to initialize thread schema")?;
            user_version = 1;
        }
        if user_version < 2 {
            conn.execute_batch(
                r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS workflow_runs (
                    id TEXT PRIMARY KEY,
                    workflow_id TEXT NOT NULL,
                    goal TEXT NOT NULL,
                    status TEXT NOT NULL,
                    input_hash TEXT,
                    started_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    metadata_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE INDEX IF NOT EXISTS idx_workflow_runs_status_started_at
                    ON workflow_runs(status, started_at DESC);
                CREATE INDEX IF NOT EXISTS idx_workflow_runs_workflow_started_at
                    ON workflow_runs(workflow_id, started_at DESC);

                CREATE TABLE IF NOT EXISTS branch_runs (
                    id TEXT PRIMARY KEY,
                    workflow_run_id TEXT NOT NULL,
                    branch_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    status TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    result_json TEXT NOT NULL DEFAULT '{}',
                    FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_branch_runs_workflow_run_id
                    ON branch_runs(workflow_run_id);
                CREATE INDEX IF NOT EXISTS idx_branch_runs_branch_id
                    ON branch_runs(branch_id);

                CREATE TABLE IF NOT EXISTS leaf_runs (
                    id TEXT PRIMARY KEY,
                    workflow_run_id TEXT NOT NULL,
                    branch_run_id TEXT,
                    leaf_id TEXT NOT NULL,
                    task_id TEXT NOT NULL,
                    input_hash TEXT,
                    status TEXT NOT NULL,
                    output_json TEXT NOT NULL DEFAULT '{}',
                    artifacts_json TEXT NOT NULL DEFAULT '[]',
                    started_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE,
                    FOREIGN KEY(branch_run_id) REFERENCES branch_runs(id) ON DELETE SET NULL
                );
                CREATE INDEX IF NOT EXISTS idx_leaf_runs_workflow_run_id
                    ON leaf_runs(workflow_run_id);
                CREATE INDEX IF NOT EXISTS idx_leaf_runs_replay_lookup
                    ON leaf_runs(workflow_run_id, leaf_id, input_hash);

                CREATE TABLE IF NOT EXISTS control_node_runs (
                    id TEXT PRIMARY KEY,
                    workflow_run_id TEXT NOT NULL,
                    node_id TEXT NOT NULL,
                    kind TEXT NOT NULL,
                    status TEXT NOT NULL,
                    selected_children_json TEXT NOT NULL DEFAULT '[]',
                    result_json TEXT NOT NULL DEFAULT '{}',
                    started_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE
                );
                CREATE INDEX IF NOT EXISTS idx_control_node_runs_workflow_run_id
                    ON control_node_runs(workflow_run_id);
                CREATE INDEX IF NOT EXISTS idx_control_node_runs_node_id
                    ON control_node_runs(node_id);

                CREATE TABLE IF NOT EXISTS teacher_candidates (
                    id TEXT PRIMARY KEY,
                    workflow_run_id TEXT NOT NULL,
                    control_node_run_id TEXT NOT NULL,
                    candidate_id TEXT NOT NULL,
                    branch_run_id TEXT,
                    score REAL,
                    passed INTEGER,
                    rationale_json TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER NOT NULL,
                    FOREIGN KEY(workflow_run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE,
                    FOREIGN KEY(control_node_run_id) REFERENCES control_node_runs(id) ON DELETE CASCADE,
                    FOREIGN KEY(branch_run_id) REFERENCES branch_runs(id) ON DELETE SET NULL
                );
                CREATE INDEX IF NOT EXISTS idx_teacher_candidates_workflow_run_id
                    ON teacher_candidates(workflow_run_id);
                CREATE INDEX IF NOT EXISTS idx_teacher_candidates_control_node_run_id
                    ON teacher_candidates(control_node_run_id);

                PRAGMA user_version = 2;
                COMMIT;
                "#,
            )
            .context("failed to initialize workflow trace schema")?;
            user_version = 2;
        }
        if user_version < 3 {
            conn.execute_batch(
                r#"
                BEGIN;
                CREATE TABLE IF NOT EXISTS thread_goals (
                    thread_id TEXT PRIMARY KEY NOT NULL,
                    goal_id TEXT NOT NULL,
                    objective TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN (
                        'active',
                        'paused',
                        'blocked',
                        'usage_limited',
                        'budget_limited',
                        'complete'
                    )),
                    token_budget INTEGER,
                    tokens_used INTEGER NOT NULL DEFAULT 0,
                    time_used_seconds INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    FOREIGN KEY(thread_id) REFERENCES threads(id) ON DELETE CASCADE
                );

                PRAGMA user_version = 3;
                COMMIT;
                "#,
            )
            .context("failed to initialize thread goal schema")?;
            user_version = 3;
        }
        if user_version < 4 {
            conn.execute_batch(
                r#"
                BEGIN;
                ALTER TABLE thread_goals
                    ADD COLUMN continuation_count INTEGER NOT NULL DEFAULT 0;

                PRAGMA user_version = 4;
                COMMIT;
                "#,
            )
            .context("failed to initialize thread goal continuation schema")?;
        }
        Ok(())
    }

    /// Insert or update thread metadata.
    ///
    /// This does **not** update `current_leaf_id`; use [`append_message`](Self::append_message)
    /// or [`set_current_leaf_id`](Self::set_current_leaf_id) for that.
    pub fn upsert_thread(&self, thread: &ThreadMetadata) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            r#"
            INSERT INTO threads (
                id, rollout_path, preview, ephemeral, model_provider, created_at, updated_at, status, path, cwd,
                cli_version, source, title, sandbox_policy, approval_mode, archived, archived_at,
                git_sha, git_branch, git_origin_url, memory_mode
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                ?18, ?19, ?20, ?21
            )
            ON CONFLICT(id) DO UPDATE SET
                rollout_path=excluded.rollout_path,
                preview=excluded.preview,
                ephemeral=excluded.ephemeral,
                model_provider=excluded.model_provider,
                created_at=excluded.created_at,
                updated_at=excluded.updated_at,
                status=excluded.status,
                path=excluded.path,
                cwd=excluded.cwd,
                cli_version=excluded.cli_version,
                source=excluded.source,
                title=excluded.title,
                sandbox_policy=excluded.sandbox_policy,
                approval_mode=excluded.approval_mode,
                archived=excluded.archived,
                archived_at=excluded.archived_at,
                git_sha=excluded.git_sha,
                git_branch=excluded.git_branch,
                git_origin_url=excluded.git_origin_url,
                memory_mode=excluded.memory_mode
            "#,
            params![
                thread.id,
                path_to_opt_string(thread.rollout_path.as_deref()),
                thread.preview,
                bool_to_i64(thread.ephemeral),
                thread.model_provider,
                thread.created_at,
                thread.updated_at,
                thread_status_to_str(&thread.status),
                path_to_opt_string(thread.path.as_deref()),
                thread.cwd.display().to_string(),
                thread.cli_version,
                session_source_to_str(&thread.source),
                thread.name,
                thread.sandbox_policy,
                thread.approval_mode,
                bool_to_i64(thread.archived),
                thread.archived_at,
                thread.git_sha,
                thread.git_branch,
                thread.git_origin_url,
                thread.memory_mode,
            ],
        )
        .context("failed to upsert thread metadata")?;

        self.append_thread_name(
            &thread.id,
            thread.name.clone(),
            thread.updated_at,
            thread.rollout_path.clone(),
        )?;
        Ok(())
    }

    /// Retrieve a single thread by its ID.
    ///
    /// Returns `None` if no thread with the given ID exists.
    pub fn get_thread(&self, id: &str) -> Result<Option<ThreadMetadata>> {
        let conn = self.conn()?;
        conn.query_row(
            r#"
            SELECT id, rollout_path, preview, ephemeral, model_provider, created_at, updated_at, status, path, cwd,
                   cli_version, source, title, sandbox_policy, approval_mode, archived, archived_at,
                   git_sha, git_branch, git_origin_url, memory_mode, current_leaf_id
            FROM threads
            WHERE id = ?1
            "#,
            params![id],
            row_to_thread,
        )
        .optional()
        .context("failed to read thread")
    }

    /// List threads ordered by most recently updated.
    ///
    /// Use [`ThreadListFilters`] to control whether archived threads are included
    /// and the maximum number of results returned.
    pub fn list_threads(&self, filters: ThreadListFilters) -> Result<Vec<ThreadMetadata>> {
        let conn = self.conn()?;
        let sql = if filters.include_archived {
            "SELECT id, rollout_path, preview, ephemeral, model_provider, created_at, updated_at, status, path, cwd, cli_version, source, title, sandbox_policy, approval_mode, archived, archived_at, git_sha, git_branch, git_origin_url, memory_mode, current_leaf_id FROM threads ORDER BY updated_at DESC LIMIT ?1"
        } else {
            "SELECT id, rollout_path, preview, ephemeral, model_provider, created_at, updated_at, status, path, cwd, cli_version, source, title, sandbox_policy, approval_mode, archived, archived_at, git_sha, git_branch, git_origin_url, memory_mode, current_leaf_id FROM threads WHERE archived = 0 ORDER BY updated_at DESC LIMIT ?1"
        };

        let mut stmt = conn.prepare(sql).context("failed to prepare list query")?;
        let limit = i64::try_from(filters.limit.unwrap_or(50)).unwrap_or(50);
        let mut rows = stmt
            .query(params![limit])
            .context("failed to query threads")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate thread rows")? {
            out.push(row_to_thread(row)?);
        }
        Ok(out)
    }

    /// Archive a thread, setting its status to [`ThreadStatus::Archived`] and
    /// recording the current timestamp.
    pub fn mark_archived(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE threads SET archived = 1, archived_at = ?2, status = ?3 WHERE id = ?1",
            params![
                id,
                Utc::now().timestamp(),
                thread_status_to_str(&ThreadStatus::Archived)
            ],
        )
        .context("failed to archive thread")?;
        Ok(())
    }

    /// Unarchive a thread, removing the archived flag and clearing `archived_at`.
    pub fn mark_unarchived(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE threads SET archived = 0, archived_at = NULL WHERE id = ?1",
            params![id],
        )
        .context("failed to unarchive thread")?;
        Ok(())
    }

    /// Permanently delete a thread and all of its associated data
    /// (messages, checkpoints, dynamic tools) via cascading foreign keys.
    pub fn delete_thread(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM threads WHERE id = ?1", params![id])
            .context("failed to delete thread")?;
        Ok(())
    }

    /// Set the memory mode for a thread.
    ///
    /// Pass `None` to clear the memory mode.
    pub fn set_thread_memory_mode(&self, id: &str, mode: Option<&str>) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE threads SET memory_mode = ?2 WHERE id = ?1",
            params![id, mode],
        )
        .context("failed to update thread memory mode")?;
        Ok(())
    }

    /// Get the memory mode configured for a thread.
    ///
    /// Returns `None` if the thread does not exist or has no memory mode set.
    pub fn get_thread_memory_mode(&self, id: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT memory_mode FROM threads WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to read thread memory mode")
        .map(Option::flatten)
    }

    /// Insert or replace the persisted goal for a thread.
    pub fn upsert_thread_goal(&self, goal: &ThreadGoalRecord) -> Result<()> {
        let conn = self.conn()?;
        let exists: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM threads WHERE id = ?1",
                params![goal.thread_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to verify thread before saving goal")?;
        if exists.is_none() {
            anyhow::bail!("thread {} not found", goal.thread_id);
        }

        conn.execute(
            r#"
            INSERT INTO thread_goals (
                thread_id, goal_id, objective, status, token_budget, tokens_used,
                time_used_seconds, continuation_count, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            ON CONFLICT(thread_id) DO UPDATE SET
                goal_id=excluded.goal_id,
                objective=excluded.objective,
                status=excluded.status,
                token_budget=excluded.token_budget,
                tokens_used=excluded.tokens_used,
                time_used_seconds=excluded.time_used_seconds,
                continuation_count=excluded.continuation_count,
                created_at=excluded.created_at,
                updated_at=excluded.updated_at
            "#,
            params![
                goal.thread_id,
                goal.goal_id,
                goal.objective,
                thread_goal_status_to_str(&goal.status),
                goal.token_budget,
                goal.tokens_used,
                goal.time_used_seconds,
                goal.continuation_count,
                goal.created_at,
                goal.updated_at,
            ],
        )
        .context("failed to upsert thread goal")?;
        Ok(())
    }

    /// Accrue additional token and wall-clock usage onto a thread's persisted goal.
    ///
    /// This is the durable, additive accounting path for the persistent goal loop: it
    /// increments `tokens_used` and `time_used_seconds` in a single atomic SQL `UPDATE`
    /// (`col = col + ?`) so concurrent accruals do not race a read-modify-write. The
    /// goal's `updated_at` is advanced to the larger of its current value and `now`,
    /// keeping the timestamp monotonic even if a stale `now` is supplied.
    ///
    /// `token_delta` and `time_delta_seconds` are added on the database side; callers
    /// should pass non-negative deltas (negative values are accepted and will decrement,
    /// which is intentionally left to the caller's discretion).
    ///
    /// Returns the updated [`ThreadGoalRecord`], or `Ok(None)` if the thread has no
    /// persisted goal. Unlike [`upsert_thread_goal`](Self::upsert_thread_goal) this never
    /// creates a goal row; it only accumulates onto an existing one.
    pub fn record_thread_goal_usage(
        &self,
        thread_id: &str,
        token_delta: i64,
        time_delta_seconds: i64,
        now: i64,
    ) -> Result<Option<ThreadGoalRecord>> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                r#"
                UPDATE thread_goals
                SET tokens_used = tokens_used + ?2,
                    time_used_seconds = time_used_seconds + ?3,
                    updated_at = MAX(updated_at, ?4)
                WHERE thread_id = ?1
                "#,
                params![thread_id, token_delta, time_delta_seconds, now],
            )
            .context("failed to record thread goal usage")?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_thread_goal(thread_id)
    }

    /// Increment the durable cross-turn continuation counter for a thread goal.
    ///
    /// The older TUI continuation guard is scoped to one engine turn. This
    /// counter is intentionally persisted so a resumed goal loop can feed
    /// `goal_loop::decide_continuation` with the true cross-turn count.
    pub fn record_thread_goal_continuation(
        &self,
        thread_id: &str,
        now: i64,
    ) -> Result<Option<ThreadGoalRecord>> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                r#"
                UPDATE thread_goals
                SET continuation_count = continuation_count + 1,
                    updated_at = MAX(updated_at, ?2)
                WHERE thread_id = ?1
                "#,
                params![thread_id, now],
            )
            .context("failed to record thread goal continuation")?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_thread_goal(thread_id)
    }

    /// Retrieve the persisted goal for a thread.
    pub fn get_thread_goal(&self, thread_id: &str) -> Result<Option<ThreadGoalRecord>> {
        let conn = self.conn()?;
        conn.query_row(
            r#"
            SELECT thread_id, goal_id, objective, status, token_budget, tokens_used,
                   time_used_seconds, continuation_count, created_at, updated_at
            FROM thread_goals
            WHERE thread_id = ?1
            "#,
            params![thread_id],
            row_to_thread_goal,
        )
        .optional()
        .context("failed to read thread goal")
    }

    /// Delete the persisted goal for a thread.
    pub fn delete_thread_goal(&self, thread_id: &str) -> Result<bool> {
        let conn = self.conn()?;
        let changed = conn
            .execute(
                "DELETE FROM thread_goals WHERE thread_id = ?1",
                params![thread_id],
            )
            .context("failed to delete thread goal")?;
        Ok(changed > 0)
    }

    /// List all leaf messages in a thread.
    ///
    /// A leaf message is one that has no other message referencing it as a parent.
    /// In a branching conversation tree, there may be multiple leaf messages.
    pub fn list_leaf_messages(&self, thread_id: &str) -> Result<Vec<MessageRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                r#"
                SELECT m1.id, m1.thread_id, m1.role, m1.content, m1.item_json, m1.created_at, m1.parent_entry_id
                FROM messages m1
                LEFT JOIN messages m2 ON m1.id = m2.parent_entry_id
                WHERE m1.thread_id = ?1 AND m2.id IS NULL
                "#,
            )
            .context("failed to prepare message listing query")?;
        let mut rows = stmt
            .query(params![thread_id])
            .with_context(|| format!("failed to list leaf messages for thread {thread_id}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate message rows")? {
            let item_json: Option<String> = row.get(4).context("failed to read item json")?;
            let item = item_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .with_context(|| {
                    format!("failed to parse message item json in thread {thread_id}")
                })?;
            out.push(MessageRecord {
                id: row.get(0).context("failed to read message id")?,
                thread_id: row.get(1).context("failed to read message thread id")?,
                role: row.get(2).context("failed to read message role")?,
                content: row.get(3).context("failed to read message content")?,
                item,
                created_at: row.get(5).context("failed to read message timestamp")?,
                parent_entry_id: row.get(6).context("failed to read parent entry id")?,
            });
        }
        Ok(out)
    }

    /// Update the current leaf message pointer for a thread.
    ///
    /// This controls which branch of the conversation tree is considered active
    /// when listing messages via [`list_messages`](Self::list_messages).
    pub fn set_current_leaf_id(&self, thread_id: &str, current_leaf_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE threads SET current_leaf_id = ?1 WHERE id = ?2",
            params![current_leaf_id, thread_id],
        )
        .context("failed to update thread current leaf id")?;
        Ok(())
    }

    /// Replace the dynamic tools for a thread.
    ///
    /// All existing dynamic tools for the thread are deleted and replaced with the
    /// provided list. The operation is performed within a transaction.
    pub fn persist_dynamic_tools(
        &self,
        thread_id: &str,
        tools: &[DynamicToolRecord],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("failed to begin dynamic tools transaction")?;
        tx.execute(
            "DELETE FROM thread_dynamic_tools WHERE thread_id = ?1",
            params![thread_id],
        )
        .context("failed to clear dynamic tools")?;
        for tool in tools {
            tx.execute(
                "INSERT INTO thread_dynamic_tools(thread_id, position, name, description, input_schema) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    thread_id,
                    tool.position,
                    tool.name,
                    tool.description,
                    tool.input_schema.to_string()
                ],
            )
            .with_context(|| format!("failed to persist dynamic tool {}", tool.name))?;
        }
        tx.commit().context("failed to commit dynamic tools")?;
        Ok(())
    }

    /// Retrieve all dynamic tools registered for a thread, ordered by position.
    pub fn get_dynamic_tools(&self, thread_id: &str) -> Result<Vec<DynamicToolRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT position, name, description, input_schema FROM thread_dynamic_tools WHERE thread_id = ?1 ORDER BY position ASC",
            )
            .context("failed to prepare get dynamic tools query")?;
        let mut rows = stmt
            .query(params![thread_id])
            .context("failed to query dynamic tools")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate dynamic tools")? {
            let input_schema_raw: String =
                row.get(3).context("failed to read tool input schema")?;
            let input_schema: Value =
                serde_json::from_str(&input_schema_raw).with_context(|| {
                    format!("failed to parse input schema for dynamic tool in thread {thread_id}")
                })?;
            out.push(DynamicToolRecord {
                position: row.get(0).context("failed to read tool position")?,
                name: row.get(1).context("failed to read tool name")?,
                description: row.get(2).context("failed to read tool description")?,
                input_schema,
            });
        }
        Ok(out)
    }

    /// Append a new message to a thread.
    ///
    /// The message is linked to the thread's current leaf as its parent, and the
    /// thread's `current_leaf_id` is updated to the new message. Returns the ID
    /// of the newly created message.
    pub fn append_message(
        &self,
        thread_id: &str,
        role: &str,
        content: &str,
        item: Option<Value>,
    ) -> Result<i64> {
        let mut conn = self.conn()?;
        let created_at = Utc::now().timestamp();
        let item_json = item
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize message item payload")?;

        let tx = conn
            .transaction()
            .context("failed to begin append message transaction")?;

        let current_leaf_id: Option<i64> = tx
            .query_row(
                "SELECT current_leaf_id FROM threads WHERE id = ?1",
                params![thread_id],
                |row| row.get(0),
            )
            .with_context(|| {
                format!("failed to query thread current leaf id for thread {thread_id}")
            })?;

        let next_leaf_id: i64 = tx.query_row(
            r#"
                INSERT INTO messages(thread_id, role, content, item_json, created_at, parent_entry_id)
                SELECT ?1, ?2, ?3, ?4, ?5, ?6
                RETURNING id
            "#, params![thread_id, role, content, item_json, created_at, current_leaf_id], |row| row.get(0)
        ).with_context(|| format!("failed to append message for thread {thread_id}"))?;

        tx.execute(
            r#"
            UPDATE threads
            SET current_leaf_id = ?1
            WHERE id = ?2;
            "#,
            params![next_leaf_id, thread_id],
        )
        .with_context(|| {
            format!("failed to update thread current leaf id for thread {thread_id}")
        })?;

        tx.commit()
            .context("failed to commit append message transaction")?;

        Ok(next_leaf_id)
    }

    /// List messages in the current conversation branch, walking backwards from
    /// the thread's `current_leaf_id`.
    ///
    /// Messages are returned in chronological order (oldest first). The `limit`
    /// parameter caps how many ancestor messages are traversed; it defaults to 500.
    pub fn list_messages(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<MessageRecord>> {
        let conn = self.conn()?;
        let limit = i64::try_from(limit.unwrap_or(500)).unwrap_or(500);
        let mut stmt = conn
            .prepare(
                r#"
                WITH RECURSIVE
                    leaf_id AS (
                        SELECT current_leaf_id FROM threads WHERE id = ?1
                    ),
                    ancestors AS (
                        SELECT id, thread_id, role, content, item_json, created_at, parent_entry_id, 0 AS depth
                        FROM messages
                        WHERE id = (SELECT current_leaf_id FROM leaf_id)

                        UNION ALL

                        SELECT m.id, m.thread_id, m.role, m.content, m.item_json, m.created_at, m.parent_entry_id, a.depth + 1
                        FROM messages m
                        JOIN ancestors a ON m.id = a.parent_entry_id
                        WHERE a.depth < ?2
                    )
                    SELECT id, thread_id, role, content, item_json, created_at, parent_entry_id FROM ancestors
                    ORDER BY depth DESC
                "#
            )
            .context("failed to prepare message listing query")?;
        let mut rows = stmt
            .query(params![thread_id, limit - 1])
            .with_context(|| format!("failed to list messages for thread {thread_id}"))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate message rows")? {
            let item_json: Option<String> = row.get(4).context("failed to read item json")?;
            let item = item_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .with_context(|| {
                    format!("failed to parse message item json in thread {thread_id}")
                })?;
            out.push(MessageRecord {
                id: row.get(0).context("failed to read message id")?,
                thread_id: row.get(1).context("failed to read message thread id")?,
                role: row.get(2).context("failed to read message role")?,
                content: row.get(3).context("failed to read message content")?,
                item,
                created_at: row.get(5).context("failed to read message timestamp")?,
                parent_entry_id: row.get(6).context("failed to read parent entry id")?,
            });
        }
        Ok(out)
    }

    /// Fork the conversation at a specific message.
    ///
    /// Creates a new message whose parent is `message_id` and updates the thread's
    /// `current_leaf_id` to the new message. Returns the ID of the new message.
    /// This enables branching conversations from any point in the history.
    pub fn fork_at_message(
        &self,
        message_id: &str,
        role: &str,
        content: &str,
        item: Option<Value>,
    ) -> Result<i64> {
        let mut conn = self.conn()?;
        let created_at = Utc::now().timestamp();
        let item_json = item
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .context("failed to serialize message item payload")?;

        let tx = conn
            .transaction()
            .context("failed to begin fork message transaction")?;

        let thread_id: String = tx
            .query_row(
                "SELECT thread_id FROM messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to query thread id for message {message_id}"))?;

        let next_leaf_id: i64 = tx.query_row(
            r#"
                INSERT INTO messages(thread_id, role, content, item_json, created_at, parent_entry_id)
                SELECT ?1, ?2, ?3, ?4, ?5, ?6
                RETURNING id
            "#, params![thread_id, role, content, item_json, created_at, message_id], |row| row.get(0)
        ).with_context(|| format!("failed to fork at message for thread {thread_id:?}"))?;

        tx.execute(
            r#"
            UPDATE threads
            SET current_leaf_id = ?1
            WHERE id = ?2;
            "#,
            params![next_leaf_id, thread_id],
        )
        .with_context(|| {
            format!("failed to update thread current leaf id for thread {thread_id:?}")
        })?;

        tx.commit()
            .context("failed to commit fork message transaction")?;

        Ok(next_leaf_id)
    }

    /// Delete all messages belonging to a thread and reset its `current_leaf_id`.
    ///
    /// Returns the number of messages deleted.
    pub fn clear_messages(&self, thread_id: &str) -> Result<usize> {
        let mut conn = self.conn()?;
        let tx = conn
            .transaction()
            .context("failed to begin clear messages transaction")?;

        tx.execute(
            r#"
            UPDATE threads
            SET current_leaf_id = NULL
            WHERE id = ?1;
            "#,
            params![thread_id],
        )
        .with_context(|| format!("failed to clear messages for thread {thread_id}"))?;
        let result = tx
            .execute(
                r#"
                DELETE FROM messages WHERE thread_id = ?1
                "#,
                params![thread_id],
            )
            .with_context(|| format!("failed to clear messages for thread {thread_id}"))?;
        tx.commit()
            .context("failed to commit clear messages transaction")?;

        Ok(result)
    }

    /// Save (or update) a named checkpoint for a thread.
    ///
    /// If a checkpoint with the same `thread_id` and `checkpoint_id` already exists,
    /// its state and timestamp are overwritten.
    pub fn save_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: &str,
        state: &Value,
    ) -> Result<()> {
        let conn = self.conn()?;
        let state_json =
            serde_json::to_string(state).context("failed to encode checkpoint state")?;
        conn.execute(
            r#"
            INSERT INTO checkpoints(thread_id, checkpoint_id, state_json, created_at)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(thread_id, checkpoint_id) DO UPDATE SET
                state_json = excluded.state_json,
                created_at = excluded.created_at
            "#,
            params![thread_id, checkpoint_id, state_json, Utc::now().timestamp()],
        )
        .with_context(|| {
            format!("failed to save checkpoint {checkpoint_id} for thread {thread_id}")
        })?;
        Ok(())
    }

    /// Load a checkpoint for a thread.
    ///
    /// If `checkpoint_id` is provided, loads that specific checkpoint. Otherwise,
    /// loads the most recently created checkpoint for the thread. Returns `None`
    /// if no matching checkpoint exists.
    pub fn load_checkpoint(
        &self,
        thread_id: &str,
        checkpoint_id: Option<&str>,
    ) -> Result<Option<CheckpointRecord>> {
        let conn = self.conn()?;
        if let Some(checkpoint_id) = checkpoint_id {
            let row = conn
                .query_row(
                    "SELECT thread_id, checkpoint_id, state_json, created_at FROM checkpoints WHERE thread_id = ?1 AND checkpoint_id = ?2",
                    params![thread_id, checkpoint_id],
                    |row| {
                        let state_json: String = row.get(2)?;
                        let state = serde_json::from_str(&state_json).unwrap_or(Value::Null);
                        Ok(CheckpointRecord {
                            thread_id: row.get(0)?,
                            checkpoint_id: row.get(1)?,
                            state,
                            created_at: row.get(3)?,
                        })
                    },
                )
                .optional()
                .with_context(|| {
                    format!("failed to load checkpoint {checkpoint_id} for thread {thread_id}")
                })?;
            return Ok(row);
        }

        conn.query_row(
            "SELECT thread_id, checkpoint_id, state_json, created_at FROM checkpoints WHERE thread_id = ?1 ORDER BY created_at DESC LIMIT 1",
            params![thread_id],
            |row| {
                let state_json: String = row.get(2)?;
                let state = serde_json::from_str(&state_json).unwrap_or(Value::Null);
                Ok(CheckpointRecord {
                    thread_id: row.get(0)?,
                    checkpoint_id: row.get(1)?,
                    state,
                    created_at: row.get(3)?,
                })
            },
        )
        .optional()
        .with_context(|| format!("failed to load latest checkpoint for thread {thread_id}"))
    }

    /// List checkpoints for a thread, ordered by creation time (newest first).
    ///
    /// The `limit` parameter caps the number of results and defaults to 100.
    pub fn list_checkpoints(
        &self,
        thread_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<CheckpointRecord>> {
        let conn = self.conn()?;
        let limit = i64::try_from(limit.unwrap_or(100)).unwrap_or(100);
        let mut stmt = conn
            .prepare(
                "SELECT thread_id, checkpoint_id, state_json, created_at FROM checkpoints WHERE thread_id = ?1 ORDER BY created_at DESC LIMIT ?2",
            )
            .context("failed to prepare checkpoint list query")?;
        let mut rows = stmt
            .query(params![thread_id, limit])
            .with_context(|| format!("failed to list checkpoints for thread {thread_id}"))?;

        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate checkpoint rows")? {
            let state_json: String = row.get(2).context("failed to read checkpoint state json")?;
            let state = serde_json::from_str(&state_json).unwrap_or(Value::Null);
            out.push(CheckpointRecord {
                thread_id: row.get(0).context("failed to read checkpoint thread id")?,
                checkpoint_id: row.get(1).context("failed to read checkpoint id")?,
                state,
                created_at: row.get(3).context("failed to read checkpoint timestamp")?,
            });
        }
        Ok(out)
    }

    /// Delete a specific checkpoint from a thread.
    pub fn delete_checkpoint(&self, thread_id: &str, checkpoint_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM checkpoints WHERE thread_id = ?1 AND checkpoint_id = ?2",
            params![thread_id, checkpoint_id],
        )
        .with_context(|| {
            format!("failed to delete checkpoint {checkpoint_id} for thread {thread_id}")
        })?;
        Ok(())
    }

    /// Insert or update a background job record.
    pub fn upsert_job(&self, job: &JobStateRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            r#"
            INSERT INTO jobs(id, name, status, progress, detail, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                status = excluded.status,
                progress = excluded.progress,
                detail = excluded.detail,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            "#,
            params![
                job.id,
                job.name,
                job_state_status_to_str(&job.status),
                job.progress.map(i64::from),
                job.detail,
                job.created_at,
                job.updated_at
            ],
        )
        .with_context(|| format!("failed to upsert job {}", job.id))?;
        Ok(())
    }

    /// Retrieve a single job by its ID.
    ///
    /// Returns `None` if no job with the given ID exists.
    pub fn get_job(&self, id: &str) -> Result<Option<JobStateRecord>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, status, progress, detail, created_at, updated_at FROM jobs WHERE id = ?1",
            params![id],
            |row| {
                let status_raw: String = row.get(2)?;
                let progress: Option<i64> = row.get(3)?;
                Ok(JobStateRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    status: job_state_status_from_str(&status_raw),
                    progress: progress.and_then(|v| u8::try_from(v).ok()),
                    detail: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .with_context(|| format!("failed to read job {id}"))
    }

    /// List jobs ordered by most recently updated.
    ///
    /// The `limit` parameter caps the number of results and defaults to 100.
    pub fn list_jobs(&self, limit: Option<usize>) -> Result<Vec<JobStateRecord>> {
        let conn = self.conn()?;
        let limit = i64::try_from(limit.unwrap_or(100)).unwrap_or(100);
        let mut stmt = conn
            .prepare(
                "SELECT id, name, status, progress, detail, created_at, updated_at FROM jobs ORDER BY updated_at DESC LIMIT ?1",
            )
            .context("failed to prepare job list query")?;
        let mut rows = stmt
            .query(params![limit])
            .context("failed to query persisted jobs")?;
        let mut out = Vec::new();
        while let Some(row) = rows.next().context("failed to iterate persisted jobs")? {
            let status_raw: String = row.get(2).context("failed to read job status")?;
            let progress: Option<i64> = row.get(3).context("failed to read job progress")?;
            out.push(JobStateRecord {
                id: row.get(0).context("failed to read job id")?,
                name: row.get(1).context("failed to read job name")?,
                status: job_state_status_from_str(&status_raw),
                progress: progress.and_then(|v| u8::try_from(v).ok()),
                detail: row.get(4).context("failed to read job detail")?,
                created_at: row.get(5).context("failed to read job created_at")?,
                updated_at: row.get(6).context("failed to read job updated_at")?,
            });
        }
        Ok(out)
    }

    /// Permanently delete a job record.
    pub fn delete_job(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM jobs WHERE id = ?1", params![id])
            .with_context(|| format!("failed to delete job {id}"))?;
        Ok(())
    }

    /// Look up the rollout file path for a thread by its ID.
    pub fn find_rollout_path_by_id(&self, id: &str) -> Result<Option<PathBuf>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            params![id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .context("failed to lookup rollout path")
        .map(|opt| opt.flatten().map(PathBuf::from))
    }

    /// Append an entry to the JSONL session index file.
    ///
    /// The session index is an append-only log that maps thread IDs to their names,
    /// update timestamps, and rollout paths. It is used for fast name-based lookups
    /// without opening the SQLite database.
    pub fn append_thread_name(
        &self,
        thread_id: &str,
        thread_name: Option<String>,
        updated_at: i64,
        rollout_path: Option<PathBuf>,
    ) -> Result<()> {
        if let Some(parent) = self.session_index_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create session index directory {}",
                    parent.display()
                )
            })?;
        }
        let entry = SessionIndexEntry {
            thread_id: thread_id.to_string(),
            thread_name,
            updated_at,
            rollout_path,
        };
        let encoded =
            serde_json::to_string(&entry).context("failed to serialize session index entry")?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.session_index_path)
            .with_context(|| {
                format!(
                    "failed to open session index {}",
                    self.session_index_path.display()
                )
            })?;
        writeln!(file, "{encoded}").context("failed to append session index entry")?;
        Ok(())
    }

    /// Find the display name for a thread by its ID, using the session index.
    ///
    /// Returns `None` if the thread is not in the index or has no name.
    pub fn find_thread_name_by_id(&self, thread_id: &str) -> Result<Option<String>> {
        let map = self.session_index_map()?;
        Ok(map
            .get(thread_id)
            .and_then(|entry| entry.thread_name.clone()))
    }

    /// Look up display names for multiple thread IDs at once.
    ///
    /// Returns a map from thread ID to its name (which may be `None`).
    pub fn find_thread_names_by_ids(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, Option<String>>> {
        let map = self.session_index_map()?;
        let mut out = HashMap::new();
        for id in ids {
            let name = map.get(id).and_then(|entry| entry.thread_name.clone());
            out.insert(id.clone(), name);
        }
        Ok(out)
    }

    /// Find the rollout path for a thread by its display name (case-insensitive).
    ///
    /// If multiple threads share the same name, the most recently updated one is returned.
    /// Returns `None` if no matching thread is found.
    pub fn find_thread_path_by_name_str(&self, name: &str) -> Result<Option<PathBuf>> {
        let map = self.session_index_map()?;
        let matched = map
            .values()
            .filter(|entry| {
                entry
                    .thread_name
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case(name))
            })
            .max_by_key(|entry| entry.updated_at);
        Ok(matched.and_then(|entry| entry.rollout_path.clone()))
    }

    fn session_index_map(&self) -> Result<HashMap<String, SessionIndexEntry>> {
        if !self.session_index_path.exists() {
            return Ok(HashMap::new());
        }
        let file = OpenOptions::new()
            .read(true)
            .open(&self.session_index_path)
            .with_context(|| {
                format!(
                    "failed to read session index {}",
                    self.session_index_path.display()
                )
            })?;
        let reader = BufReader::new(file);
        let mut latest = HashMap::<String, SessionIndexEntry>::new();
        for line in reader.lines() {
            let line = line.context("failed to read session index line")?;
            if line.trim().is_empty() {
                continue;
            }
            let parsed: SessionIndexEntry =
                serde_json::from_str(&line).context("failed to parse session index entry")?;
            latest.insert(parsed.thread_id.clone(), parsed);
        }
        Ok(latest)
    }
}

fn default_state_db_path() -> PathBuf {
    // $CODEWHALE_HOME is a hard override of the base data directory
    // (docs/CONFIGURATION.md): when set, the state DB lives under it and we do
    // NOT fall back to the legacy ~/.deepseek path — silent fallback would
    // defeat the isolation the override promises (CI, containers, multi-project,
    // test harnesses). Legacy ~/.deepseek migration only applies to the default
    // home location.
    if let Some(overridden) = codewhale_home_override() {
        return overridden.join("state.db");
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    // Prefer the CodeWhale directory, falling back to legacy DeepSeek path
    // so existing installs don't lose their session history.
    let primary = home.join(".codewhale").join("state.db");
    if primary.exists() || !home.join(".deepseek").join("state.db").exists() {
        primary
    } else {
        home.join(".deepseek").join("state.db")
    }
}

/// Resolve `$CODEWHALE_HOME` as a hard override of the data directory root.
///
/// Returns the path verbatim (the env var IS the home dir, matching
/// `codewhale_home()` in config — `$CODEWHALE_HOME=/data/cw` means the home is
/// `/data/cw`, not `/data/cw/.codewhale`). Returns `None` when unset/empty so
/// callers can branch on "explicit override" vs "default home + legacy
/// fallback." Mirrors config's helper without taking a dependency on it (state
/// is a low-level leaf crate; config cannot be a dependency here without
/// inverting the layering).
fn codewhale_home_override() -> Option<PathBuf> {
    std::env::var_os("CODEWHALE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn bool_to_i64(value: bool) -> i64 {
    if value { 1 } else { 0 }
}

fn i64_to_bool(value: i64) -> bool {
    value != 0
}

fn thread_status_to_str(status: &ThreadStatus) -> &'static str {
    match status {
        ThreadStatus::Running => "running",
        ThreadStatus::Idle => "idle",
        ThreadStatus::Completed => "completed",
        ThreadStatus::Failed => "failed",
        ThreadStatus::Paused => "paused",
        ThreadStatus::Archived => "archived",
    }
}

fn thread_status_from_str(value: &str) -> ThreadStatus {
    match value {
        "running" => ThreadStatus::Running,
        "idle" => ThreadStatus::Idle,
        "completed" => ThreadStatus::Completed,
        "failed" => ThreadStatus::Failed,
        "paused" => ThreadStatus::Paused,
        "archived" => ThreadStatus::Archived,
        _ => ThreadStatus::Idle,
    }
}

fn session_source_to_str(source: &SessionSource) -> &'static str {
    match source {
        SessionSource::Interactive => "interactive",
        SessionSource::Resume => "resume",
        SessionSource::Fork => "fork",
        SessionSource::Api => "api",
        SessionSource::Unknown => "unknown",
    }
}

fn session_source_from_str(value: &str) -> SessionSource {
    match value {
        "interactive" => SessionSource::Interactive,
        "resume" => SessionSource::Resume,
        "fork" => SessionSource::Fork,
        "api" => SessionSource::Api,
        _ => SessionSource::Unknown,
    }
}

fn path_to_opt_string(path: Option<&Path>) -> Option<String> {
    path.map(|p| p.display().to_string())
}

fn job_state_status_to_str(status: &JobStateStatus) -> &'static str {
    match status {
        JobStateStatus::Queued => "queued",
        JobStateStatus::Running => "running",
        JobStateStatus::Completed => "completed",
        JobStateStatus::Failed => "failed",
        JobStateStatus::Cancelled => "cancelled",
    }
}

fn job_state_status_from_str(value: &str) -> JobStateStatus {
    match value {
        "queued" => JobStateStatus::Queued,
        "running" => JobStateStatus::Running,
        "completed" => JobStateStatus::Completed,
        "failed" => JobStateStatus::Failed,
        "cancelled" => JobStateStatus::Cancelled,
        _ => JobStateStatus::Queued,
    }
}

fn thread_goal_status_to_str(status: &ThreadGoalStatus) -> &'static str {
    match status {
        ThreadGoalStatus::Active => "active",
        ThreadGoalStatus::Paused => "paused",
        ThreadGoalStatus::Blocked => "blocked",
        ThreadGoalStatus::UsageLimited => "usage_limited",
        ThreadGoalStatus::BudgetLimited => "budget_limited",
        ThreadGoalStatus::Complete => "complete",
    }
}

fn thread_goal_status_from_str(value: &str) -> ThreadGoalStatus {
    match value {
        "active" => ThreadGoalStatus::Active,
        "paused" => ThreadGoalStatus::Paused,
        "blocked" => ThreadGoalStatus::Blocked,
        "usage_limited" => ThreadGoalStatus::UsageLimited,
        "budget_limited" => ThreadGoalStatus::BudgetLimited,
        "complete" => ThreadGoalStatus::Complete,
        _ => ThreadGoalStatus::Active,
    }
}

fn row_to_thread(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadMetadata> {
    let status_raw: String = row.get(7)?;
    let source_raw: String = row.get(11)?;
    let rollout_path: Option<String> = row.get(1)?;
    let path: Option<String> = row.get(8)?;
    Ok(ThreadMetadata {
        id: row.get(0)?,
        rollout_path: rollout_path.map(PathBuf::from),
        preview: row.get(2)?,
        ephemeral: i64_to_bool(row.get(3)?),
        model_provider: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        status: thread_status_from_str(&status_raw),
        path: path.map(PathBuf::from),
        cwd: PathBuf::from(row.get::<_, String>(9)?),
        cli_version: row.get(10)?,
        source: session_source_from_str(&source_raw),
        name: row.get(12)?,
        sandbox_policy: row.get(13)?,
        approval_mode: row.get(14)?,
        archived: i64_to_bool(row.get(15)?),
        archived_at: row.get(16)?,
        git_sha: row.get(17)?,
        git_branch: row.get(18)?,
        git_origin_url: row.get(19)?,
        memory_mode: row.get(20)?,
        current_leaf_id: row.get(21)?,
    })
}

fn row_to_thread_goal(row: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadGoalRecord> {
    let status_raw: String = row.get(3)?;
    Ok(ThreadGoalRecord {
        thread_id: row.get(0)?,
        goal_id: row.get(1)?,
        objective: row.get(2)?,
        status: thread_goal_status_from_str(&status_raw),
        token_budget: row.get(4)?,
        tokens_used: row.get(5)?,
        time_used_seconds: row.get(6)?,
        continuation_count: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_state_store(name: &str) -> StateStore {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "codewhale-state-{name}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp state dir");
        StateStore::open(Some(dir.join("state.db"))).expect("open state store")
    }

    fn test_thread(id: &str) -> ThreadMetadata {
        ThreadMetadata {
            id: id.to_string(),
            rollout_path: None,
            preview: "test thread".to_string(),
            ephemeral: false,
            model_provider: "deepseek".to_string(),
            created_at: 10,
            updated_at: 10,
            status: ThreadStatus::Running,
            path: None,
            cwd: PathBuf::from("/tmp/codewhale"),
            cli_version: "0.0.0-test".to_string(),
            source: SessionSource::Interactive,
            name: None,
            sandbox_policy: None,
            approval_mode: None,
            archived: false,
            archived_at: None,
            git_sha: None,
            git_branch: None,
            git_origin_url: None,
            memory_mode: None,
            current_leaf_id: None,
        }
    }

    fn test_goal(thread_id: &str, objective: &str) -> ThreadGoalRecord {
        ThreadGoalRecord {
            thread_id: thread_id.to_string(),
            goal_id: "goal-1".to_string(),
            objective: objective.to_string(),
            status: ThreadGoalStatus::Active,
            token_budget: Some(123),
            tokens_used: 7,
            time_used_seconds: 11,
            continuation_count: 0,
            created_at: 100,
            updated_at: 101,
        }
    }

    #[test]
    fn thread_goal_crud_round_trips_and_replaces() {
        let store = temp_state_store("thread-goal-crud");
        store
            .upsert_thread(&test_thread("thread-1"))
            .expect("upsert thread");

        let goal = test_goal("thread-1", "Ship v0.8.59");
        store.upsert_thread_goal(&goal).expect("upsert goal");
        assert_eq!(
            store
                .get_thread_goal("thread-1")
                .expect("read goal")
                .as_ref(),
            Some(&goal)
        );

        let mut replacement = test_goal("thread-1", "Ship v0.8.59 safely");
        replacement.goal_id = "goal-2".to_string();
        replacement.status = ThreadGoalStatus::BudgetLimited;
        replacement.token_budget = None;
        replacement.updated_at = 202;
        store
            .upsert_thread_goal(&replacement)
            .expect("replace goal");
        assert_eq!(
            store.get_thread_goal("thread-1").expect("read replacement"),
            Some(replacement)
        );

        assert!(store.delete_thread_goal("thread-1").expect("delete goal"));
        assert!(
            store
                .get_thread_goal("thread-1")
                .expect("read empty")
                .is_none()
        );
        assert!(!store.delete_thread_goal("thread-1").expect("delete empty"));
    }

    #[test]
    fn thread_goal_requires_existing_thread() {
        let store = temp_state_store("thread-goal-missing-thread");
        let err = store
            .upsert_thread_goal(&test_goal("missing-thread", "nope"))
            .expect_err("goal without a thread should fail");
        assert!(err.to_string().contains("thread missing-thread not found"));
    }

    #[test]
    fn delete_thread_cascades_child_rows() {
        let store = temp_state_store("delete-thread-cascades");
        store
            .upsert_thread(&test_thread("thread-1"))
            .expect("upsert thread");
        store
            .append_message("thread-1", "user", "hello", Some(serde_json::json!({})))
            .expect("append message");
        store
            .save_checkpoint(
                "thread-1",
                "checkpoint-1",
                &serde_json::json!({ "ok": true }),
            )
            .expect("save checkpoint");

        store.delete_thread("thread-1").expect("delete thread");

        let conn = store.conn().expect("conn");
        let message_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE thread_id = ?1",
                params!["thread-1"],
                |row| row.get(0),
            )
            .expect("count messages");
        let checkpoint_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM checkpoints WHERE thread_id = ?1",
                params!["thread-1"],
                |row| row.get(0),
            )
            .expect("count checkpoints");

        assert_eq!(message_count, 0);
        assert_eq!(checkpoint_count, 0);
    }

    #[test]
    fn record_thread_goal_usage_accumulates_tokens_and_time() {
        let store = temp_state_store("thread-goal-usage");
        store
            .upsert_thread(&test_thread("thread-1"))
            .expect("upsert thread");

        // Mirror the runtime, which creates goals with zeroed accounting.
        let mut goal = test_goal("thread-1", "Ship the persistent goal loop");
        goal.tokens_used = 0;
        goal.time_used_seconds = 0;
        goal.updated_at = 100;
        store.upsert_thread_goal(&goal).expect("upsert goal");

        // First accrual lands the deltas and advances updated_at.
        let after_first = store
            .record_thread_goal_usage("thread-1", 250, 12, 150)
            .expect("record usage")
            .expect("goal exists");
        assert_eq!(after_first.tokens_used, 250);
        assert_eq!(after_first.time_used_seconds, 12);
        assert_eq!(after_first.updated_at, 150);
        // Identity fields are preserved across accrual.
        assert_eq!(after_first.goal_id, goal.goal_id);
        assert_eq!(after_first.objective, goal.objective);
        assert_eq!(after_first.status, goal.status);
        assert_eq!(after_first.token_budget, goal.token_budget);
        assert_eq!(after_first.created_at, goal.created_at);
        assert_eq!(after_first.continuation_count, 0);

        // Second accrual adds on top of the first (additive, not replacing).
        let after_second = store
            .record_thread_goal_usage("thread-1", 75, 8, 200)
            .expect("record usage")
            .expect("goal exists");
        assert_eq!(after_second.tokens_used, 325);
        assert_eq!(after_second.time_used_seconds, 20);
        assert_eq!(after_second.updated_at, 200);

        // A stale `now` must not move updated_at backwards.
        let after_stale = store
            .record_thread_goal_usage("thread-1", 5, 1, 1)
            .expect("record usage")
            .expect("goal exists");
        assert_eq!(after_stale.tokens_used, 330);
        assert_eq!(after_stale.time_used_seconds, 21);
        assert_eq!(after_stale.updated_at, 200);

        // Read back through the normal getter to confirm durability.
        let persisted = store
            .get_thread_goal("thread-1")
            .expect("read goal")
            .expect("goal exists");
        assert_eq!(persisted.tokens_used, 330);
        assert_eq!(persisted.time_used_seconds, 21);
    }

    #[test]
    fn record_thread_goal_usage_returns_none_without_goal() {
        let store = temp_state_store("thread-goal-usage-missing");
        store
            .upsert_thread(&test_thread("thread-1"))
            .expect("upsert thread");
        // Thread exists but has no goal row yet: accrual is a no-op, not an error,
        // and must not create a goal.
        let result = store
            .record_thread_goal_usage("thread-1", 100, 5, 999)
            .expect("record usage on goalless thread");
        assert!(result.is_none());
        assert!(
            store
                .get_thread_goal("thread-1")
                .expect("read goal")
                .is_none()
        );
    }

    #[test]
    fn record_thread_goal_continuation_accumulates_durably() {
        let store = temp_state_store("thread-goal-continuation");
        store
            .upsert_thread(&test_thread("thread-1"))
            .expect("upsert thread");

        let mut goal = test_goal("thread-1", "Keep working across turns");
        goal.updated_at = 100;
        store.upsert_thread_goal(&goal).expect("upsert goal");

        let after_first = store
            .record_thread_goal_continuation("thread-1", 120)
            .expect("record continuation")
            .expect("goal exists");
        assert_eq!(after_first.continuation_count, 1);
        assert_eq!(after_first.tokens_used, goal.tokens_used);
        assert_eq!(after_first.time_used_seconds, goal.time_used_seconds);
        assert_eq!(after_first.updated_at, 120);

        let after_second = store
            .record_thread_goal_continuation("thread-1", 110)
            .expect("record second continuation")
            .expect("goal exists");
        assert_eq!(after_second.continuation_count, 2);
        assert_eq!(after_second.updated_at, 120);

        let persisted = store
            .get_thread_goal("thread-1")
            .expect("read goal")
            .expect("goal exists");
        assert_eq!(persisted.continuation_count, 2);
    }

    // ── $CODEWHALE_HOME override tests ──────────────────────────────
    //
    // These touch a process-global env var, so they serialize against each
    // other (and restore the prior value) to stay hermetic under parallel test
    // runs — the same concern AGENTS.md flags for config_command_allow_shell_*.

    static CODEWHALE_HOME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct CodeWhaleHomeGuard {
        prior: Option<std::ffi::OsString>,
    }
    impl CodeWhaleHomeGuard {
        fn set(value: &str) -> Self {
            let prior = std::env::var_os("CODEWHALE_HOME");
            // SAFETY: serialised by CODEWHALE_HOME_TEST_LOCK.
            unsafe { std::env::set_var("CODEWHALE_HOME", value) };
            Self { prior }
        }
        fn remove() -> Self {
            let prior = std::env::var_os("CODEWHALE_HOME");
            // SAFETY: serialised by CODEWHALE_HOME_TEST_LOCK.
            unsafe { std::env::remove_var("CODEWHALE_HOME") };
            Self { prior }
        }
    }
    impl Drop for CodeWhaleHomeGuard {
        fn drop(&mut self) {
            // SAFETY: serialised by CODEWHALE_HOME_TEST_LOCK.
            unsafe {
                match &self.prior {
                    Some(value) => std::env::set_var("CODEWHALE_HOME", value),
                    None => std::env::remove_var("CODEWHALE_HOME"),
                }
            }
        }
    }

    #[test]
    fn codewhale_home_override_returns_the_env_value_verbatim() {
        let _lock = CODEWHALE_HOME_TEST_LOCK.lock().unwrap();
        let _g = CodeWhaleHomeGuard::set("/tmp/cw-isolated-state");
        // The env var IS the home dir — no ".codewhale" appended. This matches
        // codewhale_home() in config ($CODEWHALE_HOME=/x means home is /x).
        assert_eq!(
            codewhale_home_override().as_deref(),
            Some(std::path::Path::new("/tmp/cw-isolated-state"))
        );
    }

    #[test]
    fn codewhale_home_override_none_when_unset() {
        let _lock = CODEWHALE_HOME_TEST_LOCK.lock().unwrap();
        let _g = CodeWhaleHomeGuard::remove();
        assert!(codewhale_home_override().is_none());
    }

    #[test]
    fn codewhale_home_override_none_when_empty() {
        let _lock = CODEWHALE_HOME_TEST_LOCK.lock().unwrap();
        let _g = CodeWhaleHomeGuard::set("   ");
        // The helper filters empty values (after the OsString check). Note:
        // var_os returns the raw "   ", and our filter only catches truly-empty,
        // so this documents that whitespace-only is NOT treated as unset at the
        // override layer (config's codewhale_home trims; we don't here — the
        // branch is "was it set at all").
        assert!(
            codewhale_home_override().is_some(),
            "non-empty (even whitespace) counts as set; trimming is the caller's job"
        );
    }

    #[test]
    fn default_state_db_path_uses_codewhale_home_when_set() {
        let _lock = CODEWHALE_HOME_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!(
            "cw-home-state-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _g = CodeWhaleHomeGuard::set(dir.to_str().unwrap());
        // Hard override: the DB is <CODEWHALE_HOME>/state.db, NOT
        // <CODEWHALE_HOME>/.codewhale/state.db, and the legacy ~/.deepseek
        // fallback is bypassed entirely.
        assert_eq!(default_state_db_path(), dir.join("state.db"));
    }
}
