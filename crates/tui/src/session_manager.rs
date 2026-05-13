//! Session management for resuming conversations.
//!
//! This module provides functionality for:
//! - Saving sessions to disk
//! - Listing previous sessions
//! - Resuming sessions by ID
//! - Managing session lifecycle

use crate::artifacts::ArtifactRecord;
use crate::models::{ContentBlock, Message, SystemPrompt};
use crate::tui::file_mention::ContextReference;
use crate::utils::write_atomic;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Maximum number of sessions to retain
const MAX_SESSIONS: usize = 50;
/// Maximum number of messages to persist per session (#402 P0).
/// Beyond this limit, the oldest messages are dropped and a truncation
/// note is prepended to the system prompt. Keeps session files bounded
/// so save/load remains fast even for long-running conversations.
const MAX_PERSISTED_MESSAGES: usize = 500;
const CURRENT_SESSION_SCHEMA_VERSION: u32 = 1;
const CURRENT_QUEUE_SCHEMA_VERSION: u32 = 1;

const fn default_session_schema_version() -> u32 {
    CURRENT_SESSION_SCHEMA_VERSION
}

const fn default_queue_schema_version() -> u32 {
    CURRENT_QUEUE_SCHEMA_VERSION
}

fn normalize_managed_dir(path: PathBuf) -> std::io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed directory path cannot be empty",
        ));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) && path.is_relative()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed directory path cannot contain traversal components",
        ));
    }
    if path.is_absolute() {
        return Ok(path);
    }
    std::env::current_dir().map(|cwd| cwd.join(path))
}

/// Persisted queued message for offline/degraded mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedSessionMessage {
    pub display: String,
    #[serde(default)]
    pub skill_instruction: Option<String>,
}

/// Persisted queue state for recovery after restart/crash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineQueueState {
    #[serde(default = "default_queue_schema_version")]
    pub schema_version: u32,
    /// Session ID this queue belongs to. Queue is only restored when
    /// resuming the same session to prevent stale messages leaking into new chats.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub messages: Vec<QueuedSessionMessage>,
    #[serde(default)]
    pub draft: Option<QueuedSessionMessage>,
}

impl Default for OfflineQueueState {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_QUEUE_SCHEMA_VERSION,
            session_id: None,
            messages: Vec::new(),
            draft: None,
        }
    }
}

/// Durable context-reference metadata attached to a user message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionContextReference {
    pub message_index: usize,
    pub reference: ContextReference,
}

/// Session metadata stored with each saved session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Unique session identifier
    pub id: String,
    /// Human-readable title (derived from first message)
    pub title: String,
    /// When the session was created
    pub created_at: DateTime<Utc>,
    /// When the session was last updated
    pub updated_at: DateTime<Utc>,
    /// Number of messages in the session
    pub message_count: usize,
    /// Total tokens used
    pub total_tokens: u64,
    /// Model used for the session
    pub model: String,
    /// Workspace directory
    pub workspace: PathBuf,
    /// Optional mode label (agent/plan/etc.)
    #[serde(default)]
    pub mode: Option<String>,
    /// Accumulated cost data for persisted billing and high-water mark.
    #[serde(default)]
    pub cost: SessionCostSnapshot,
}

/// Cost and high-water-mark fields persisted with each session.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SessionCostSnapshot {
    /// Accumulated parent-turn session cost in USD.
    #[serde(default)]
    pub session_cost_usd: f64,
    /// Accumulated parent-turn session cost in CNY.
    #[serde(default)]
    pub session_cost_cny: f64,
    /// Accumulated sub-agent/background LLM cost in USD.
    #[serde(default)]
    pub subagent_cost_usd: f64,
    /// Accumulated sub-agent/background LLM cost in CNY.
    #[serde(default)]
    pub subagent_cost_cny: f64,
    /// Max-ever displayed session+subagent cost in USD (preserves #244
    /// monotonic guarantee across session restarts).
    #[serde(default)]
    pub displayed_cost_high_water_usd: f64,
    /// Max-ever displayed session+subagent cost in CNY.
    #[serde(default)]
    pub displayed_cost_high_water_cny: f64,
}

impl SessionCostSnapshot {
    /// Session + subagent cost in USD.
    pub fn total_usd(&self) -> f64 {
        self.session_cost_usd + self.subagent_cost_usd
    }

    /// Session + subagent cost in CNY.
    pub fn total_cny(&self) -> f64 {
        self.session_cost_cny + self.subagent_cost_cny
    }
}

impl SessionMetadata {
    /// Copy cost fields from another metadata (used when forking a session).
    #[allow(dead_code)]
    pub fn copy_cost_from(&mut self, other: &SessionMetadata) {
        self.cost = other.cost;
    }
}

/// A saved session containing full conversation history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    /// Schema version for migration compatibility
    #[serde(default = "default_session_schema_version")]
    pub schema_version: u32,
    /// Session metadata
    pub metadata: SessionMetadata,
    /// Conversation messages
    pub messages: Vec<Message>,
    /// System prompt if any
    pub system_prompt: Option<String>,
    /// Compact linked context references for user-visible `@path` and
    /// `/attach` mentions. Optional for backward-compatible session loads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_references: Vec<SessionContextReference>,
    /// Metadata registry of large outputs produced during this session.
    /// Artifact contents are stored in the session-owned artifact directory.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<ArtifactRecord>,
}

/// Manager for session persistence operations
#[derive(Debug)]
pub struct SessionManager {
    /// Directory where sessions are stored
    sessions_dir: PathBuf,
}

impl SessionManager {
    fn validated_session_path(&self, id: &str) -> std::io::Result<PathBuf> {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Session id cannot be empty",
            ));
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Invalid session id '{id}'"),
            ));
        }
        Ok(self.sessions_dir.join(format!("{trimmed}.json")))
    }

    /// Create a new `SessionManager` with the specified sessions directory
    pub fn new(sessions_dir: PathBuf) -> std::io::Result<Self> {
        let sessions_dir = normalize_managed_dir(sessions_dir)?;
        // Ensure the sessions directory exists
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }

    /// Create a `SessionManager` using the default location (~/.deepseek/sessions)
    pub fn default_location() -> std::io::Result<Self> {
        Self::new(default_sessions_dir()?)
    }

    /// Save a session to disk using atomic write (temp file + fsync + rename).
    pub fn save_session(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let path = self.validated_session_path(&session.metadata.id)?;

        let content = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Atomic write via write_atomic (NamedTempFile + fsync + persist)
        write_atomic(&path, content.as_bytes())?;

        // Clean up old sessions if we have too many
        self.cleanup_old_sessions()?;

        Ok(path)
    }

    /// Save a crash-recovery checkpoint for in-flight turns.
    pub fn save_checkpoint(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let checkpoints = self.sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoints)?;
        let path = checkpoints.join("latest.json");
        let content = serde_json::to_string_pretty(session)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(&path, content.as_bytes())?;
        Ok(path)
    }

    /// Load the most recent crash-recovery checkpoint if present.
    pub fn load_checkpoint(&self) -> std::io::Result<Option<SavedSession>> {
        let path = self.sessions_dir.join("checkpoints").join("latest.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let session: SavedSession = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if session.schema_version > CURRENT_SESSION_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Checkpoint schema v{} is newer than supported v{}",
                    session.schema_version, CURRENT_SESSION_SCHEMA_VERSION
                ),
            ));
        }
        Ok(Some(session))
    }

    /// Clear any crash-recovery checkpoint.
    pub fn clear_checkpoint(&self) -> std::io::Result<()> {
        let path = self.sessions_dir.join("checkpoints").join("latest.json");
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Save offline queue state (queued + draft messages).
    pub fn save_offline_queue_state(
        &self,
        state: &OfflineQueueState,
        session_id: Option<&str>,
    ) -> std::io::Result<PathBuf> {
        let checkpoints = self.sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoints)?;
        let path = checkpoints.join("offline_queue.json");
        let mut state_with_id = state.clone();
        state_with_id.session_id = session_id.map(|s| s.to_string());
        let content = serde_json::to_string_pretty(&state_with_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        write_atomic(&path, content.as_bytes())?;
        Ok(path)
    }

    /// Load offline queue state if present.
    pub fn load_offline_queue_state(&self) -> std::io::Result<Option<OfflineQueueState>> {
        let path = self
            .sessions_dir
            .join("checkpoints")
            .join("offline_queue.json");
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(&path)?;
        let state: OfflineQueueState = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if state.schema_version > CURRENT_QUEUE_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Offline queue schema v{} is newer than supported v{}",
                    state.schema_version, CURRENT_QUEUE_SCHEMA_VERSION
                ),
            ));
        }
        Ok(Some(state))
    }

    /// Remove persisted offline queue state.
    pub fn clear_offline_queue_state(&self) -> std::io::Result<()> {
        let path = self
            .sessions_dir
            .join("checkpoints")
            .join("offline_queue.json");
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Load a session by ID
    pub fn load_session(&self, id: &str) -> std::io::Result<SavedSession> {
        let path = self.validated_session_path(id)?;

        let content = fs::read_to_string(&path)?;
        let session: SavedSession = serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if session.schema_version > CURRENT_SESSION_SCHEMA_VERSION {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Session schema v{} is newer than supported v{}",
                    session.schema_version, CURRENT_SESSION_SCHEMA_VERSION
                ),
            ));
        }

        Ok(session)
    }

    /// Load a session by partial ID prefix
    pub fn load_session_by_prefix(&self, prefix: &str) -> std::io::Result<SavedSession> {
        let sessions = self.list_sessions()?;

        let matches: Vec<_> = sessions
            .into_iter()
            .filter(|s| s.id.starts_with(prefix))
            .collect();

        match matches.len() {
            0 => Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("No session found with prefix: {prefix}"),
            )),
            1 => self.load_session(&matches[0].id),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "Ambiguous prefix '{}' matches {} sessions",
                    prefix,
                    matches.len()
                ),
            )),
        }
    }

    /// List all saved sessions, sorted by most recently updated
    pub fn list_sessions(&self) -> std::io::Result<Vec<SessionMetadata>> {
        let mut sessions = Vec::new();

        for entry in fs::read_dir(&self.sessions_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "json")
                && let Ok(session) = Self::load_session_metadata(&path)
            {
                sessions.push(session);
            }
        }

        // Sort by updated_at descending (most recent first)
        sessions.sort_by_key(|s| std::cmp::Reverse(s.updated_at));

        Ok(sessions)
    }

    /// Load only the metadata from a session file.
    ///
    /// Optimization for #337: previously this called
    /// `serde_json::from_reader` which forces serde to scan every token in
    /// the file just to validate JSON structure — including the
    /// (potentially many MB of) `messages` and `tool_log` arrays we're
    /// going to discard. For a user with hundreds of long sessions, a
    /// single `list_sessions()` call could chew through tens of MB of
    /// JSON per startup.
    ///
    /// We now read at most 64 KB up front and string-extract the
    /// top-level `metadata` object, which is invariably tiny (~500 B)
    /// and appears before any large `messages`/`tool_log` payload. We
    /// fall back to a full-file read only if the prefix doesn't yield a
    /// parseable metadata block (e.g. an oddly-formatted legacy file).
    fn load_session_metadata(path: &Path) -> std::io::Result<SessionMetadata> {
        use std::io::Read;

        const PREFIX_BYTES: usize = 64 * 1024;
        let mut file = fs::File::open(path)?;
        let mut buf = Vec::with_capacity(PREFIX_BYTES);
        file.by_ref()
            .take(PREFIX_BYTES as u64)
            .read_to_end(&mut buf)?;

        if let Some(metadata) = extract_top_level_metadata(&buf) {
            return Ok(metadata);
        }

        // Metadata wasn't extractable from the prefix (truncated mid-block,
        // unusual key ordering, etc.). Read the rest and try again with the
        // full buffer before giving up.
        let mut rest = Vec::new();
        file.read_to_end(&mut rest)?;
        buf.extend_from_slice(&rest);
        extract_top_level_metadata(&buf).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "session file missing parseable `metadata` block",
            )
        })
    }

    /// Delete a session by ID
    pub fn delete_session(&self, id: &str) -> std::io::Result<()> {
        let path = self.validated_session_path(id)?;
        fs::remove_file(path)?;
        let session_dir = self.sessions_dir.join(id.trim());
        if session_dir.exists() {
            fs::remove_dir_all(session_dir)?;
        }
        Ok(())
    }

    /// Clean up old sessions to stay within `MAX_SESSIONS` limit
    fn cleanup_old_sessions(&self) -> std::io::Result<()> {
        let sessions = self.list_sessions()?;

        if sessions.len() > MAX_SESSIONS {
            // Delete oldest sessions
            for session in sessions.iter().skip(MAX_SESSIONS) {
                let _ = self.delete_session(&session.id);
            }
        }

        Ok(())
    }

    /// Remove session files whose `updated_at` is older than `max_age`
    /// from the persisted-sessions directory. Returns the number of
    /// records pruned. Building block for #406's phase-2 auto-archive
    /// on boot; today the user-facing entry point is the
    /// `/sessions prune <days>` slash command.
    ///
    /// Crash-recovery safety: skips the running checkpoint
    /// (`checkpoints/latest.json`) and any file under `checkpoints/`
    /// — those are owned by the checkpoint subsystem and live with
    /// stricter durability rules. Only top-level `<session_id>.json`
    /// files are candidates.
    ///
    /// `max_age` is checked against the metadata's `updated_at`
    /// timestamp embedded in the JSON, not the filesystem mtime — the
    /// user may have rsynced their `~/.deepseek` between machines and
    /// fs mtimes can lie.
    pub fn prune_sessions_older_than(
        &self,
        max_age: std::time::Duration,
    ) -> std::io::Result<usize> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::days(365 * 10));
        let sessions = self.list_sessions()?;
        let mut pruned = 0usize;
        for session in sessions {
            if session.updated_at < cutoff {
                if let Err(err) = self.delete_session(&session.id) {
                    tracing::warn!(
                        target: "session",
                        session = session.id,
                        ?err,
                        "session prune skipped a record",
                    );
                    continue;
                }
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// Get the most recent session scoped to the current workspace.
    pub fn get_latest_session_for_workspace(
        &self,
        workspace: &Path,
    ) -> std::io::Result<Option<SessionMetadata>> {
        let sessions = self.list_sessions()?;
        Ok(sessions.into_iter().find(|session| {
            workspace_scope_matches(&session.workspace, workspace)
                && !is_empty_auto_created_session(session)
        }))
    }

    /// Search sessions by title
    pub fn search_sessions(&self, query: &str) -> std::io::Result<Vec<SessionMetadata>> {
        let query_lower = query.to_lowercase();
        let sessions = self.list_sessions()?;

        Ok(sessions
            .into_iter()
            .filter(|s| s.title.to_lowercase().contains(&query_lower))
            .collect())
    }
}

pub(crate) fn workspace_scope_matches(saved_workspace: &Path, current_workspace: &Path) -> bool {
    if paths_equivalent(saved_workspace, current_workspace) {
        return true;
    }

    match (
        find_git_root(saved_workspace),
        find_git_root(current_workspace),
    ) {
        (Some(saved_root), Some(current_root)) => paths_equivalent(&saved_root, &current_root),
        _ => false,
    }
}

fn is_empty_auto_created_session(session: &SessionMetadata) -> bool {
    session.message_count == 0 && session.title.trim().eq_ignore_ascii_case("New Session")
}

fn paths_equivalent(lhs: &Path, rhs: &Path) -> bool {
    let lhs_canonical = fs::canonicalize(lhs).ok();
    let rhs_canonical = fs::canonicalize(rhs).ok();
    match (lhs_canonical, rhs_canonical) {
        (Some(lhs), Some(rhs)) => lhs == rhs,
        _ => lhs == rhs,
    }
}

fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    loop {
        if is_git_metadata_entry(&current.join(".git")) {
            return Some(current);
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return None,
        }
    }
}

fn is_git_metadata_entry(path: &Path) -> bool {
    if path.is_dir() {
        return path.join("HEAD").is_file();
    }

    fs::read_to_string(path)
        .map(|content| content.trim_start().starts_with("gitdir:"))
        .unwrap_or(false)
}

/// Resolve the default session directory path (`~/.deepseek/sessions`).
pub fn default_sessions_dir() -> std::io::Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "Home directory not found")
    })?;
    Ok(home.join(".deepseek").join("sessions"))
}

/// Prune snapshots older than `max_age` for `workspace`.
///
/// Always non-fatal. Returns silently — callers don't need the count
/// (the underlying repo logs at WARN if anything blew up).
pub fn prune_workspace_snapshots(workspace: &Path, max_age: std::time::Duration) {
    match crate::snapshot::prune_older_than(workspace, max_age) {
        Ok(0) => {}
        Ok(n) => {
            tracing::debug!(target: "snapshot", "boot prune removed {n} snapshot(s)");
        }
        Err(e) => {
            tracing::warn!(target: "snapshot", "boot prune failed: {e}");
        }
    }
}

/// Create a new `SavedSession` from conversation state
pub fn create_saved_session(
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
) -> SavedSession {
    create_saved_session_with_mode(
        messages,
        model,
        workspace,
        total_tokens,
        system_prompt,
        None,
    )
}

/// Create a new `SavedSession` from conversation state with optional mode label
pub fn create_saved_session_with_mode(
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
    mode: Option<&str>,
) -> SavedSession {
    create_saved_session_with_id_and_mode(
        Uuid::new_v4().to_string(),
        messages,
        model,
        workspace,
        total_tokens,
        system_prompt,
        mode,
    )
}

/// Create a new `SavedSession` using a caller-owned session id.
pub fn create_saved_session_with_id_and_mode(
    id: String,
    messages: &[Message],
    model: &str,
    workspace: &Path,
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
    mode: Option<&str>,
) -> SavedSession {
    let now = Utc::now();

    // Generate title from first user message
    let title = messages
        .iter()
        .find(|m| m.role == "user")
        .and_then(|m| {
            m.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => {
                    let prompt = extract_user_prompt(text);
                    if prompt.is_empty() {
                        None
                    } else {
                        Some(truncate_title(prompt, 50))
                    }
                }
                _ => None,
            })
        })
        .unwrap_or_else(|| "New Session".to_string());

    let (capped_messages, truncation_note) = cap_messages(messages);

    SavedSession {
        schema_version: CURRENT_SESSION_SCHEMA_VERSION,
        metadata: SessionMetadata {
            id,
            title,
            created_at: now,
            updated_at: now,
            message_count: messages.len(),
            total_tokens,
            model: model.to_string(),
            workspace: workspace.to_path_buf(),
            mode: mode.map(str::to_string),
            cost: SessionCostSnapshot::default(),
        },
        messages: capped_messages,
        system_prompt: merge_truncation_note(
            system_prompt_to_string(system_prompt),
            truncation_note,
        ),
        context_references: Vec::new(),
        artifacts: Vec::new(),
    }
}

/// Update an existing session with new messages
pub fn update_session(
    mut session: SavedSession,
    messages: &[Message],
    total_tokens: u64,
    system_prompt: Option<&SystemPrompt>,
) -> SavedSession {
    session.schema_version = CURRENT_SESSION_SCHEMA_VERSION;
    let (capped_messages, truncation_note) = cap_messages(messages);
    session.messages = capped_messages;
    session.metadata.updated_at = Utc::now();
    session.metadata.message_count = messages.len();
    session.metadata.total_tokens = total_tokens;
    session.system_prompt = merge_truncation_note(
        system_prompt_to_string(system_prompt).or(session.system_prompt),
        truncation_note,
    );
    session
}

/// Cap messages to [`MAX_PERSISTED_MESSAGES`], keeping the most recent.
/// Returns the capped slice and an optional truncation note.
fn cap_messages(messages: &[Message]) -> (Vec<Message>, Option<String>) {
    let total = messages.len();
    if total <= MAX_PERSISTED_MESSAGES {
        return (messages.to_vec(), None);
    }
    let dropped = total - MAX_PERSISTED_MESSAGES;
    let note = format!(
        "Note: {dropped} older messages were dropped from the session file \
         to keep persistence bounded. The full conversation history may \
         still be recoverable from cycle archives."
    );
    (
        messages[total - MAX_PERSISTED_MESSAGES..].to_vec(),
        Some(note),
    )
}

/// Merge an optional truncation note into the system prompt string.
fn merge_truncation_note(system_prompt: Option<String>, note: Option<String>) -> Option<String> {
    match (system_prompt, note) {
        (None, None) => None,
        (Some(sp), None) => Some(sp),
        (None, Some(note)) => Some(format!("[Session note]\n{note}")),
        (Some(sp), Some(note)) => Some(format!("[Session note]\n{note}\n\n---\n\n{sp}")),
    }
}

/// String-scan a JSON byte buffer for the top-level `"metadata":{...}`
/// block and return it parsed. Returns `None` if no balanced metadata
/// object is present in the buffer.
///
/// Supports the optimisation in `SessionManager::load_session_metadata`
/// (#337). The scanner is brace-balanced and string-aware so a `{` or
/// `}` appearing inside a string literal doesn't perturb the depth
/// count.
fn extract_top_level_metadata(buf: &[u8]) -> Option<SessionMetadata> {
    let s = std::str::from_utf8(buf).ok()?;
    let bytes = s.as_bytes();

    // Find the FIRST `"metadata"` key that appears outside of any string
    // literal. Walking with brace/string awareness costs almost nothing
    // and avoids matching `metadata` inside an earlier message body.
    let key_pat = b"\"metadata\"";
    let mut idx = 0usize;
    let mut in_string = false;
    let mut escape = false;
    let key_offset = loop {
        if idx >= bytes.len() {
            return None;
        }
        let c = bytes[idx];
        if escape {
            escape = false;
            idx += 1;
            continue;
        }
        if c == b'\\' {
            escape = true;
            idx += 1;
            continue;
        }
        if c == b'"' {
            // If we're already in a string, this closes it; otherwise it
            // opens one. But before flipping we check for the key match
            // when we're entering a string at exactly this position.
            if !in_string && bytes[idx..].starts_with(key_pat) {
                break idx;
            }
            in_string = !in_string;
            idx += 1;
            continue;
        }
        idx += 1;
    };

    // Position past the key.
    let after_key = key_offset + key_pat.len();
    // Find the colon that separates key from value (skip whitespace).
    let mut after_colon = after_key;
    while after_colon < bytes.len() && (bytes[after_colon] as char).is_whitespace() {
        after_colon += 1;
    }
    if after_colon >= bytes.len() || bytes[after_colon] != b':' {
        return None;
    }
    after_colon += 1;
    while after_colon < bytes.len() && (bytes[after_colon] as char).is_whitespace() {
        after_colon += 1;
    }
    if after_colon >= bytes.len() || bytes[after_colon] != b'{' {
        return None;
    }

    // Walk the object, balancing braces.
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    let mut end = None;
    for (i, &c) in bytes[after_colon..].iter().enumerate() {
        let abs = after_colon + i;
        if escape {
            escape = false;
            continue;
        }
        if c == b'\\' {
            escape = true;
            continue;
        }
        if c == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match c {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(abs + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end?;
    serde_json::from_str::<SessionMetadata>(&s[after_colon..end]).ok()
}

fn system_prompt_to_string(system_prompt: Option<&SystemPrompt>) -> Option<String> {
    match system_prompt {
        Some(SystemPrompt::Text(text)) => Some(text.clone()),
        Some(SystemPrompt::Blocks(blocks)) => Some(
            blocks
                .iter()
                .map(|b| b.text.clone())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n"),
        ),
        None => None,
    }
}

/// Truncate a session ID to 8 characters for compact display.
/// Returns a `&str` borrowing from the input — no allocation.
pub fn truncate_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Strip a leading `<turn_meta>...</turn_meta>` block from saved user text.
///
/// Older sessions can have turn metadata prefixed to the first user message.
/// The session picker and generated session titles should show the user's
/// prompt, not the cache/debug envelope.
pub(crate) fn extract_user_prompt(raw: &str) -> &str {
    let trimmed = raw.trim_start();
    let Some(after_open) = trimmed.strip_prefix("<turn_meta>") else {
        return trimmed;
    };
    if let Some(close_pos) = after_open.find("</turn_meta>") {
        return after_open[close_pos + "</turn_meta>".len()..].trim_start();
    }
    after_open.trim_start()
}

/// Clean a stored title for display, falling back to a neutral label.
pub(crate) fn extract_title(raw: &str) -> &str {
    let title = extract_user_prompt(raw);
    if title.is_empty() { "Session" } else { title }
}

/// Strip common inline thinking/reasoning XML sections from saved assistant
/// text before it is shown in session previews.
pub(crate) fn strip_thinking_tags(text: &str) -> String {
    if !text.contains("<think") && !text.contains("<thinking") && !text.contains("<reasoning") {
        return text.to_string();
    }

    let tags = ["think", "thinking", "reasoning"];
    let mut result = text.to_string();
    for tag in tags {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        while let Some(start) = result.find(&open) {
            let Some(end) = result[start..].find(&close) else {
                break;
            };
            let end_abs = start + end + close.len();
            result.replace_range(start..end_abs, "");
        }
    }
    result
}

/// Truncate a string to create a title (character-safe for UTF-8)
fn truncate_title(s: &str, max_len: usize) -> String {
    let s = s.trim();
    let first_line = s.lines().next().unwrap_or(s);

    let char_count = first_line.chars().count();
    if char_count <= max_len {
        first_line.to_string()
    } else {
        let truncated: String = first_line.chars().take(max_len - 3).collect();
        format!("{truncated}...")
    }
}

/// Format a session for display in a picker
pub fn format_session_line(meta: &SessionMetadata) -> String {
    let age = format_age(&meta.updated_at);
    let truncated_title = truncate_title(extract_title(&meta.title), 40);

    format!(
        "{} | {} | {} msgs | {}",
        truncate_id(&meta.id),
        truncated_title,
        meta.message_count,
        age
    )
}

/// Format a datetime as relative age
fn format_age(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    if duration.num_minutes() < 1 {
        "just now".to_string()
    } else if duration.num_hours() < 1 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_days() < 1 {
        format!("{}h ago", duration.num_hours())
    } else if duration.num_weeks() < 1 {
        format!("{}d ago", duration.num_days())
    } else {
        format!("{}w ago", duration.num_weeks())
    }
}

// === Unit Tests ===

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ContentBlock;
    use std::fs;
    use tempfile::tempdir;

    fn make_test_message(role: &str, text: &str) -> Message {
        Message {
            role: role.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    fn write_session_record(
        manager: &SessionManager,
        id: &str,
        workspace: &Path,
        updated_at: DateTime<Utc>,
    ) {
        let session = SavedSession {
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
            messages: vec![make_test_message("user", "hi")],
            metadata: SessionMetadata {
                id: id.to_string(),
                title: format!("session-{id}"),
                created_at: updated_at,
                updated_at,
                message_count: 1,
                total_tokens: 0,
                model: "deepseek-v4-flash".to_string(),
                workspace: workspace.to_path_buf(),
                mode: None,
                cost: SessionCostSnapshot::default(),
            },
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
        };
        manager.save_session(&session).expect("save");
    }

    fn write_empty_session_record(
        manager: &SessionManager,
        id: &str,
        workspace: &Path,
        updated_at: DateTime<Utc>,
    ) {
        let session = SavedSession {
            schema_version: CURRENT_SESSION_SCHEMA_VERSION,
            messages: Vec::new(),
            metadata: SessionMetadata {
                id: id.to_string(),
                title: "New Session".to_string(),
                created_at: updated_at,
                updated_at,
                message_count: 0,
                total_tokens: 0,
                model: "deepseek-v4-pro".to_string(),
                workspace: workspace.to_path_buf(),
                mode: Some("yolo".to_string()),
                cost: SessionCostSnapshot::default(),
            },
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
        };
        manager.save_session(&session).expect("save empty");
    }

    #[test]
    fn test_session_manager_new() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        assert!(tmp.path().join("sessions").exists());
        let _ = manager;
    }

    #[test]
    fn test_save_and_load_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![
            make_test_message("user", "Hello!"),
            make_test_message("assistant", "Hi there!"),
        ];

        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");

        let loaded = manager.load_session(&session_id).expect("load");
        assert_eq!(loaded.metadata.id, session_id);
        assert_eq!(loaded.messages.len(), 2);
    }

    #[test]
    fn test_list_sessions() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        // Create a few sessions
        for i in 0..3 {
            let messages = vec![make_test_message("user", &format!("Session {i}"))];
            let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
            manager.save_session(&session).expect("save");
        }

        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn latest_session_for_workspace_ignores_newer_other_directory() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace_a = tmp.path().join("aa").join("aaa");
        let workspace_b = tmp.path().join("bb").join("bbb");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace b");

        write_session_record(
            &manager,
            "current-workspace",
            &workspace_a,
            Utc::now() - chrono::Duration::minutes(10),
        );
        write_session_record(&manager, "other-workspace", &workspace_b, Utc::now());

        let global = manager
            .list_sessions()
            .expect("list")
            .into_iter()
            .next()
            .expect("global latest");
        assert_eq!(global.id, "other-workspace");

        let scoped = manager
            .get_latest_session_for_workspace(&workspace_a)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "current-workspace");
    }

    #[test]
    fn latest_session_for_workspace_ignores_invalid_parent_git_marker() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace_a = tmp.path().join("aa").join("aaa");
        let workspace_b = tmp.path().join("bb").join("bbb");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace b");
        fs::create_dir_all(tmp.path().join(".git")).expect("mkdir invalid git marker");

        write_session_record(
            &manager,
            "current-workspace",
            &workspace_a,
            Utc::now() - chrono::Duration::minutes(10),
        );
        write_session_record(&manager, "other-workspace", &workspace_b, Utc::now());

        let scoped = manager
            .get_latest_session_for_workspace(&workspace_a)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "current-workspace");
    }

    #[test]
    fn latest_session_for_workspace_matches_same_git_repository() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let repo = tmp.path().join("repo");
        let repo_app = repo.join("apps").join("client");
        let repo_crate = repo.join("crates").join("server");
        let other_repo = tmp.path().join("other").join("project");
        fs::create_dir_all(repo.join(".git")).expect("mkdir .git");
        fs::write(repo.join(".git").join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
        fs::create_dir_all(&repo_app).expect("mkdir repo app");
        fs::create_dir_all(&repo_crate).expect("mkdir repo crate");
        fs::create_dir_all(&other_repo).expect("mkdir other repo");

        write_session_record(
            &manager,
            "same-repo",
            &repo_app,
            Utc::now() - chrono::Duration::minutes(5),
        );
        write_session_record(&manager, "other-repo", &other_repo, Utc::now());

        let scoped = manager
            .get_latest_session_for_workspace(&repo_crate)
            .expect("latest for workspace")
            .expect("same repo latest");
        assert_eq!(scoped.id, "same-repo");
    }

    #[test]
    fn latest_session_for_workspace_skips_empty_auto_created_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace = tmp.path().join("repo");
        fs::create_dir_all(&workspace).expect("mkdir workspace");

        write_session_record(
            &manager,
            "interrupted-user-turn",
            &workspace,
            Utc::now() - chrono::Duration::minutes(5),
        );
        write_empty_session_record(&manager, "empty-auto-shell", &workspace, Utc::now());

        let global = manager
            .list_sessions()
            .expect("list")
            .into_iter()
            .next()
            .expect("global latest");
        assert_eq!(global.id, "empty-auto-shell");

        let scoped = manager
            .get_latest_session_for_workspace(&workspace)
            .expect("latest for workspace")
            .expect("scoped latest");
        assert_eq!(scoped.id, "interrupted-user-turn");
    }

    #[test]
    fn test_load_by_prefix() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![make_test_message("user", "Test session")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let prefix = truncate_id(&session.metadata.id).to_string();
        manager.save_session(&session).expect("save");

        let loaded = manager.load_session_by_prefix(&prefix).expect("load");
        assert_eq!(loaded.messages.len(), 1);
    }

    #[test]
    fn test_delete_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let messages = vec![make_test_message("user", "To be deleted")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");
        assert!(manager.load_session(&session_id).is_ok());

        manager.delete_session(&session_id).expect("delete");
        assert!(manager.load_session(&session_id).is_err());
    }

    #[test]
    fn delete_session_removes_artifact_directory() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");

        let session = create_saved_session(
            &[make_test_message("user", "artifact session")],
            "test-model",
            tmp.path(),
            100,
            None,
        );
        let session_id = session.metadata.id.clone();
        let artifact_dir = sessions_dir.join(&session_id).join("artifacts");
        fs::create_dir_all(&artifact_dir).expect("artifact dir");
        fs::write(artifact_dir.join("art_call.txt"), "raw output").expect("artifact file");

        manager.save_session(&session).expect("save");
        manager.delete_session(&session_id).expect("delete");

        assert!(!sessions_dir.join(format!("{session_id}.json")).exists());
        assert!(!sessions_dir.join(&session_id).exists());
    }

    #[test]
    fn test_session_id_rejects_invalid_characters() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let err = manager
            .load_session("../outside")
            .expect_err("invalid id should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);

        let err = manager
            .delete_session("sess bad")
            .expect_err("invalid id should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_session_manager_rejects_relative_traversal_dir() {
        let err = SessionManager::new(PathBuf::from("../sessions"))
            .expect_err("relative traversal directory should fail");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_truncate_title() {
        assert_eq!(truncate_title("Short", 50), "Short");
        assert_eq!(
            truncate_title("This is a very long title that should be truncated", 20),
            "This is a very lo..."
        );
        assert_eq!(truncate_title("Line 1\nLine 2", 50), "Line 1");
    }

    #[test]
    fn extract_user_prompt_strips_turn_meta_prefix() {
        assert_eq!(
            extract_user_prompt("<turn_meta>{\"cache\":\"x\"}</turn_meta>\nReal prompt"),
            "Real prompt"
        );
        assert_eq!(extract_user_prompt("  Real prompt"), "Real prompt");
        assert_eq!(
            extract_user_prompt("<turn_meta>{\"unterminated\":true}\nReal prompt"),
            "{\"unterminated\":true}\nReal prompt"
        );
    }

    #[test]
    fn create_saved_session_uses_prompt_after_turn_meta_for_title() {
        let tmp = tempdir().expect("tempdir");
        let messages = vec![make_test_message(
            "user",
            "<turn_meta>{\"cache\":\"x\"}</turn_meta>\nFix the session picker history pane",
        )];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        assert_eq!(
            session.metadata.title,
            "Fix the session picker history pane"
        );
    }

    #[test]
    fn strip_thinking_tags_removes_common_inline_blocks() {
        let text = "Before <think>private</think> middle <reasoning>hidden</reasoning> after";
        let cleaned = strip_thinking_tags(text);
        assert_eq!(cleaned, "Before  middle  after");
        assert_eq!(strip_thinking_tags("plain answer"), "plain answer");
    }

    #[test]
    fn test_format_age() {
        let now = Utc::now();
        assert_eq!(format_age(&now), "just now");

        let hour_ago = now - chrono::Duration::hours(2);
        assert_eq!(format_age(&hour_ago), "2h ago");

        let day_ago = now - chrono::Duration::days(3);
        assert_eq!(format_age(&day_ago), "3d ago");
    }

    #[test]
    fn test_update_session() {
        let tmp = tempdir().expect("tempdir");

        let messages = vec![make_test_message("user", "Hello")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 50, None);

        let new_messages = vec![
            make_test_message("user", "Hello"),
            make_test_message("assistant", "Hi!"),
        ];

        let updated = update_session(session, &new_messages, 100, None);
        assert_eq!(updated.messages.len(), 2);
        assert_eq!(updated.metadata.total_tokens, 100);
    }

    #[test]
    fn test_checkpoint_round_trip_and_clear() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![make_test_message("user", "checkpoint me")];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 12, None);

        manager.save_checkpoint(&session).expect("save checkpoint");
        let loaded = manager
            .load_checkpoint()
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded.metadata.id, session.metadata.id);

        manager.clear_checkpoint().expect("clear checkpoint");
        assert!(
            manager
                .load_checkpoint()
                .expect("load checkpoint")
                .is_none()
        );
    }

    #[test]
    fn workspace_scope_matches_subdirectories_in_same_git_checkout() {
        let tmp = tempdir().expect("tempdir");
        let repo = tmp.path().join("repo");
        let nested = repo.join("crates").join("tui");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::write(repo.join(".git"), "gitdir: .git/worktrees/repo").expect("write git marker");

        assert!(workspace_scope_matches(&repo, &nested));
    }

    #[test]
    fn workspace_scope_rejects_sibling_git_checkouts() {
        let tmp = tempdir().expect("tempdir");
        let first = tmp.path().join("repo-a");
        let second = tmp.path().join("repo-b");
        fs::create_dir_all(&first).expect("mkdir first");
        fs::create_dir_all(&second).expect("mkdir second");
        fs::write(first.join(".git"), "gitdir: .git/worktrees/a").expect("write first marker");
        fs::write(second.join(".git"), "gitdir: .git/worktrees/b").expect("write second marker");

        assert!(!workspace_scope_matches(&first, &second));
    }

    #[test]
    fn test_offline_queue_round_trip_and_clear() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "queued message".to_string(),
                skill_instruction: Some("Use skill".to_string()),
            }],
            draft: Some(QueuedSessionMessage {
                display: "draft message".to_string(),
                skill_instruction: None,
            }),
            ..OfflineQueueState::default()
        };

        manager
            .save_offline_queue_state(&state, Some("test-session"))
            .expect("save queue state");
        let loaded = manager
            .load_offline_queue_state()
            .expect("load queue state")
            .expect("queue state exists");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.messages[0].display, "queued message");
        assert!(loaded.draft.is_some());

        manager
            .clear_offline_queue_state()
            .expect("clear queue state");
        assert!(
            manager
                .load_offline_queue_state()
                .expect("load queue state")
                .is_none()
        );
    }

    #[test]
    fn test_offline_queue_stamps_session_id_on_save() {
        // #487: save_offline_queue_state must stamp the supplied
        // session id so the load path's mismatch check has something
        // to compare against. A queue persisted without a session id
        // is the legacy unscoped form which the load path treats as
        // stale-risky and refuses to restore.
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");

        let state = OfflineQueueState {
            messages: vec![QueuedSessionMessage {
                display: "first parked".to_string(),
                skill_instruction: None,
            }],
            ..OfflineQueueState::default()
        };

        manager
            .save_offline_queue_state(&state, Some("session-A"))
            .expect("save with session id");
        let loaded = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert_eq!(loaded.session_id.as_deref(), Some("session-A"));

        // Re-saving with a different session id replaces the stamp.
        manager
            .save_offline_queue_state(&state, Some("session-B"))
            .expect("re-save");
        let reloaded = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert_eq!(reloaded.session_id.as_deref(), Some("session-B"));

        // Saving without a session id explicitly (None) clears the
        // stamp — UI's load path treats that as legacy-unscoped and
        // fails closed.
        manager
            .save_offline_queue_state(&state, None)
            .expect("save without session id");
        let unscoped = manager
            .load_offline_queue_state()
            .expect("ok")
            .expect("present");
        assert!(
            unscoped.session_id.is_none(),
            "save with None must persist a missing session_id"
        );
    }

    #[test]
    fn test_session_context_references_round_trip() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "read @src/main.rs")],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
        );
        session.context_references.push(SessionContextReference {
            message_index: 0,
            reference: ContextReference {
                kind: crate::tui::file_mention::ContextReferenceKind::File,
                source: crate::tui::file_mention::ContextReferenceSource::AtMention,
                badge: "file".to_string(),
                label: "src/main.rs".to_string(),
                target: tmp.path().join("src/main.rs").display().to_string(),
                included: true,
                expanded: true,
                detail: Some("included".to_string()),
            },
        });

        let path = manager.save_session(&session).expect("save session");
        let loaded = manager
            .load_session(&session.metadata.id)
            .expect("load session");
        assert!(path.exists());
        assert_eq!(loaded.context_references, session.context_references);
    }

    #[test]
    fn test_checkpoint_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let checkpoints = tmp.path().join("sessions").join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("create checkpoints dir");
        let path = checkpoints.join("latest.json");
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "metadata": {
                    "id": "sid",
                    "title": "bad",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0,
                    "model": "m",
                    "workspace": "/tmp",
                    "mode": null
                },
                "messages": [],
                "system_prompt": null
            }"#,
        )
        .expect("write checkpoint");

        let err = manager.load_checkpoint().expect_err("should reject schema");
        assert!(err.to_string().contains("newer than supported"));
    }

    #[test]
    fn test_load_session_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");

        let id = "future-session";
        let path = sessions_dir.join(format!("{id}.json"));
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "metadata": {
                    "id": "future-session",
                    "title": "future",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z",
                    "message_count": 0,
                    "total_tokens": 0,
                    "model": "m",
                    "workspace": "/tmp",
                    "mode": null
                },
                "messages": [],
                "system_prompt": null
            }"#,
        )
        .expect("write session");

        let err = manager.load_session(id).expect_err("should reject schema");
        assert!(
            err.to_string().contains("newer than supported"),
            "unexpected error: {err}"
        );
    }

    /// Regression for #337: metadata extraction skips the (potentially
    /// huge) `messages` array — it must succeed even when the messages
    /// array is megabytes long, and it must NOT confuse a `"metadata"`
    /// substring inside a message body for the real top-level key.
    #[test]
    fn extract_top_level_metadata_skips_huge_messages_array() {
        // Build a session JSON with a large `messages` payload that
        // contains the literal string `"metadata"` in a user message —
        // a naive `find("\"metadata\"")` would mis-target this.
        let big_text = format!(
            r#"this message references "metadata" inside it, repeated:{}"#,
            "x".repeat(20_000)
        );
        let json = format!(
            r#"{{
                "schema_version": 1,
                "metadata": {{
                    "id": "abc-123",
                    "title": "Real Session",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-02T00:00:00Z",
                    "message_count": 12,
                    "total_tokens": 4096,
                    "model": "deepseek-v4-flash",
                    "workspace": "/tmp"
                }},
                "messages": [
                    {{ "role": "user", "content": [ {{ "Text": {{ "text": {body:?} }} }} ] }}
                ]
            }}"#,
            body = big_text
        );

        let extracted =
            extract_top_level_metadata(json.as_bytes()).expect("metadata extractable from prefix");
        assert_eq!(extracted.id, "abc-123");
        assert_eq!(extracted.title, "Real Session");
        assert_eq!(extracted.message_count, 12);
        assert_eq!(extracted.total_tokens, 4096);
    }

    #[test]
    fn extract_top_level_metadata_handles_braces_inside_strings() {
        // A title containing `{` and `}` inside the metadata block must
        // not throw off the brace counter.
        let json = r#"{
            "metadata": {
                "id": "x",
                "title": "weird { title } with braces",
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "m",
                "workspace": "/tmp"
            },
            "messages": []
        }"#;
        let extracted = extract_top_level_metadata(json.as_bytes())
            .expect("brace-in-string survives the scanner");
        assert_eq!(extracted.title, "weird { title } with braces");
    }

    #[test]
    fn saved_session_deserializes_without_artifacts_as_empty_registry() {
        let json = r#"{
            "schema_version": 1,
            "metadata": {
                "id": "legacy-session",
                "title": "legacy",
                "created_at": "2026-05-08T00:00:00Z",
                "updated_at": "2026-05-08T00:00:00Z",
                "message_count": 0,
                "total_tokens": 0,
                "model": "deepseek-v4-pro",
                "workspace": "/tmp"
            },
            "messages": [],
            "system_prompt": null
        }"#;

        let session: SavedSession = serde_json::from_str(json).expect("legacy session loads");
        assert!(session.artifacts.is_empty());
    }

    #[test]
    fn save_and_load_session_preserves_artifact_metadata() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "run tests")],
            "deepseek-v4-pro",
            Path::new("/tmp"),
            0,
            None,
        );
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call_big".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-big".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: 512_000,
            preview: "cargo test output".to_string(),
            storage_path: PathBuf::from("/tmp/tool_outputs/call-big.txt"),
        });

        manager.save_session(&session).expect("save");
        let loaded = manager.load_session(&session.metadata.id).expect("load");

        assert_eq!(loaded.artifacts, session.artifacts);
    }

    // ---- #406 prune_sessions_older_than ----
    //
    // The helper is a building block for the auto-archive design: it
    // removes session files older than a threshold while leaving fresh
    // ones (and the checkpoint directory) alone. Tests cover the empty
    // case, the all-fresh case, the all-stale case, and the mixed case.

    fn write_session_with_updated_at(
        manager: &SessionManager,
        id: &str,
        updated_at: DateTime<Utc>,
    ) {
        // Build a minimal SavedSession by hand so the test isn't tied
        // to whatever the helper functions emit; we just need a
        // metadata block whose `updated_at` matches the requested
        // value.
        write_session_record(manager, id, Path::new("/tmp"), updated_at);
    }

    #[test]
    fn prune_sessions_older_than_returns_zero_for_empty_dir() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(3600))
            .expect("prune");
        assert_eq!(pruned, 0);
    }

    #[test]
    fn prune_sessions_older_than_keeps_fresh_records() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // All updated within the last hour.
        write_session_with_updated_at(
            &manager,
            "fresh-1",
            Utc::now() - chrono::Duration::minutes(30),
        );
        write_session_with_updated_at(
            &manager,
            "fresh-2",
            Utc::now() - chrono::Duration::minutes(5),
        );
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(3600))
            .expect("prune");
        assert_eq!(pruned, 0);
        // Both files still on disk.
        assert_eq!(manager.list_sessions().expect("list").len(), 2);
    }

    #[test]
    fn prune_sessions_older_than_removes_stale_records() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // Two stale records ≥7 days old.
        write_session_with_updated_at(&manager, "stale-1", Utc::now() - chrono::Duration::days(8));
        write_session_with_updated_at(&manager, "stale-2", Utc::now() - chrono::Duration::days(30));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 2);
        assert_eq!(manager.list_sessions().expect("list").len(), 0);
    }

    #[test]
    fn prune_sessions_older_than_only_removes_stale_records_in_mixed_dir() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        write_session_with_updated_at(&manager, "fresh", Utc::now() - chrono::Duration::hours(1));
        write_session_with_updated_at(&manager, "stale", Utc::now() - chrono::Duration::days(60));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 1);
        let remaining = manager.list_sessions().expect("list");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "fresh");
    }

    #[test]
    fn prune_sessions_older_than_skips_checkpoint_directory() {
        // The checkpoint subsystem owns `<sessions>/checkpoints/` —
        // prune must not walk into it. The list_sessions iterator
        // already filters to top-level `*.json` files (skipping
        // sub-directories), so this test pins that behaviour.
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let checkpoint_dir = sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoint_dir).expect("mkdir checkpoints");
        // Drop a stale-looking JSON inside the checkpoint dir; prune
        // should leave it alone.
        let checkpoint_file = checkpoint_dir.join("latest.json");
        fs::write(&checkpoint_file, "{}").expect("write checkpoint");

        write_session_with_updated_at(&manager, "stale", Utc::now() - chrono::Duration::days(60));
        let pruned = manager
            .prune_sessions_older_than(std::time::Duration::from_secs(7 * 24 * 3600))
            .expect("prune");
        assert_eq!(pruned, 1, "the top-level stale session should be removed");
        assert!(
            checkpoint_file.exists(),
            "checkpoint file should be untouched"
        );
    }

    #[test]
    fn test_load_offline_queue_rejects_newer_schema() {
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let checkpoints = sessions_dir.join("checkpoints");
        fs::create_dir_all(&checkpoints).expect("create checkpoints dir");
        let path = checkpoints.join("offline_queue.json");
        fs::write(
            &path,
            r#"{
                "schema_version": 999,
                "messages": [],
                "draft": null
            }"#,
        )
        .expect("write queue");

        let err = manager
            .load_offline_queue_state()
            .expect_err("should reject schema");
        assert!(
            err.to_string().contains("newer than supported"),
            "unexpected error: {err}"
        );
    }
}
