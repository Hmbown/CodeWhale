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
    /// If `path` is `None`, the default location (`~/.deepseek/state.db`) is used.
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
        Connection::open(&self.db_path)
            .with_context(|| format!("failed to open state db {}", self.db_path.display()))
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
        ).with_context(|| format!("failed to fork at message for thread {:?}", thread_id))?;

        tx.execute(
            r#"
            UPDATE threads
            SET current_leaf_id = ?1
            WHERE id = ?2;
            "#,
            params![next_leaf_id, thread_id],
        )
        .with_context(|| {
            format!(
                "failed to update thread current leaf id for thread {:?}",
                thread_id
            )
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
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deepseek")
        .join("state.db")
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
