//! Session management for resuming conversations.
//!
//! This module provides functionality for:
//! - Saving sessions to disk
//! - Listing previous sessions
//! - Resuming sessions by ID
//! - Managing session lifecycle

use crate::artifacts::ArtifactRecord;
use crate::config::ApiProvider;
use crate::model_routing::AutoRouteReceipt;
use crate::models::{ContentBlock, Message, SystemPrompt};
use crate::tools::plan::PlanSnapshot;
use crate::tools::todo::TodoListSnapshot;
use crate::tui::file_mention::ContextReference;
use crate::utils::write_atomic;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use uuid::Uuid;

/// Maximum number of sessions to retain
const MAX_SESSIONS: usize = 50;
const WORK_GRAPH_IMPORT_ARCHIVE_DIR: &str = ".work-graph-import-archive";
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
    #[serde(default)]
    pub skill_provenance: Option<crate::plugins::types::PluginAuthority>,
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
    /// Provider used for the session model. Defaults for legacy saved sessions.
    #[serde(default = "default_model_provider")]
    pub model_provider: String,
    /// Exact configured provider key. This is separate from `model_provider`
    /// so old consumers can keep treating that field as the built-in provider
    /// kind (`custom` for every named custom route).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_provider_id: Option<String>,
    /// Workspace directory
    pub workspace: PathBuf,
    /// Optional mode label (agent/plan/etc.)
    #[serde(default)]
    pub mode: Option<String>,
    /// Accumulated cost data for persisted billing and high-water mark.
    #[serde(default)]
    pub cost: SessionCostSnapshot,
    /// Source session id when this session was created with `deepseek fork`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    /// Source message count at fork time. This is intentionally coarse:
    /// current saved sessions are linear JSON files, not per-entry trees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_message_count: Option<usize>,
    /// Cumulative turn duration in seconds (sum of completed turn elapsed
    /// times). Persisted so the footer "worked" chip survives restarts
    /// (#2038).
    #[serde(default)]
    pub cumulative_turn_secs: u64,
}

fn default_model_provider() -> String {
    "deepseek".to_string()
}

impl SessionMetadata {
    pub(crate) fn set_model_provider_route(&mut self, kind: &str, identity: Option<&str>) {
        self.model_provider = kind.to_string();
        self.model_provider_id = identity.map(str::to_string);
    }
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

    /// Record additive lineage metadata for a forked saved session.
    pub fn mark_forked_from(&mut self, parent: &SessionMetadata) {
        self.parent_session_id = Some(parent.id.clone());
        self.forked_from_message_count = Some(parent.message_count);
    }
}

/// Durable Work-panel state. Optional on [`SavedSession`] so every session
/// written before v0.8.68 remains loadable without migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SessionWorkState {
    /// Authoritative Work Graph. Optional so pre-Work-Graph sessions and old
    /// binaries continue to exchange fully populated Plan/To-do views.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<crate::work_graph::WorkGraphSnapshot>,
    #[serde(default, skip_serializing_if = "TodoListSnapshot::is_empty")]
    pub todos: TodoListSnapshot,
    #[serde(default, skip_serializing_if = "PlanSnapshot::is_empty")]
    pub plan: PlanSnapshot,
}

impl SessionWorkState {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.graph
            .as_ref()
            .is_none_or(crate::work_graph::WorkGraphSnapshot::is_empty)
            && self.todos.is_empty()
            && self.plan.is_empty()
    }
}

/// Latest concrete Auto route and the decision receipt that produced it.
///
/// This is additive, optional session metadata: sessions written before
/// v0.9.1 deserialize with no receipt and keep their legacy restore behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SavedAutoRouteReceipt {
    pub(crate) provider: ApiProvider,
    pub(crate) provider_identity: String,
    pub(crate) model: String,
    pub(crate) receipt: AutoRouteReceipt,
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
    /// To-do and plan state shown in the Work sidebar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_state: Option<SessionWorkState>,
    /// Most recent accepted/completed Auto decision, when the saved model mode
    /// is `auto`. Optional for backward-compatible session loads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_auto_route: Option<SavedAutoRouteReceipt>,
    /// Process-local privacy provenance. It is skipped on disk so the values
    /// being protected never become a second durable secret registry.
    #[serde(skip)]
    pub(crate) sensitive_user_input_provenance:
        crate::runtime_threads::SensitiveUserInputProvenance,
}

impl SavedSession {
    /// Rebuild the process-local privacy provenance available in this saved
    /// history without persisting a second copy of any protected value.
    fn refresh_sensitive_user_input_provenance(&self) {
        let mut sensitive_values = self.sensitive_user_input_provenance.snapshot();
        crate::runtime_threads::collect_sensitive_user_input_values(
            &self.messages,
            &mut sensitive_values,
        );
        self.sensitive_user_input_provenance
            .extend(sensitive_values);
    }
}

/// Manager for session persistence operations
#[derive(Debug)]
pub struct SessionManager {
    /// Directory where sessions are stored
    sessions_dir: PathBuf,
}

/// Origin of a crash-recovery checkpoint file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckpointSource {
    /// Per-session checkpoint file `checkpoints/<session_id>.json`.
    Session(String),
    /// Legacy single-slot checkpoint file `checkpoints/latest.json`.
    Legacy,
}

/// A crash-recovery checkpoint file discovered on disk (metadata only —
/// callers load the session content separately).
#[derive(Debug, Clone)]
pub struct CheckpointRef {
    pub source: CheckpointSource,
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
}

/// File names in `checkpoints/` that are never per-session checkpoints.
const LEGACY_CHECKPOINT_FILE: &str = "latest.json";
const OFFLINE_QUEUE_FILE: &str = "offline_queue.json";

impl SessionManager {
    fn reproject_owned_artifact_files(
        session: &SavedSession,
        sensitive_values: &std::collections::HashSet<String>,
    ) -> std::io::Result<std::collections::HashMap<(String, PathBuf), u64>> {
        let mut projected_sizes = std::collections::HashMap::new();
        for artifact in &session.artifacts {
            let owner = if artifact.session_id.is_empty() {
                session.metadata.id.as_str()
            } else {
                artifact.session_id.as_str()
            };
            // Forked sessions retain references to parent-owned artifacts.
            // Saving the fork must never rewrite the parent's durable bytes.
            if owner != session.metadata.id {
                continue;
            }
            let Some(path) =
                crate::artifacts::session_artifact_absolute_path(owner, &artifact.storage_path)
            else {
                // Absolute/legacy external paths are references, not files
                // owned or published by SavedSession persistence.
                continue;
            };
            if !path.exists() {
                continue;
            }
            let metadata = fs::metadata(&path).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "could not inspect session artifact {} for privacy projection: {error}",
                        path.display()
                    ),
                )
            })?;
            let key = (owner.to_string(), artifact.storage_path.clone());
            // Codewhale's text spill writer owns `.txt`; media/PDF fetches use
            // their validated binary extension. Never decode or rewrite an
            // arbitrary binary artifact merely because its record is attached
            // to a session.
            if !artifact
                .storage_path
                .extension()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|extension| extension.eq_ignore_ascii_case("txt"))
            {
                projected_sizes.insert(key, metadata.len());
                continue;
            }
            let bytes = fs::read(&path).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!(
                        "could not read textual session artifact {} for privacy projection: {error}",
                        path.display()
                    ),
                )
            })?;
            let Ok(raw) = String::from_utf8(bytes) else {
                // A mislabeled/legacy `.txt` artifact can still contain opaque
                // bytes. Preserve it byte-for-byte rather than failing a save
                // or corrupting the artifact.
                projected_sizes.insert(key, metadata.len());
                continue;
            };
            let projected =
                crate::runtime_threads::redacted_sensitive_user_input_text(&raw, sensitive_values);
            if projected != raw {
                write_atomic(&path, projected.as_bytes())?;
            }
            projected_sizes.insert(key, projected.len() as u64);
        }
        Ok(projected_sizes)
    }

    fn project_saved_session_for_persistence(
        session: &SavedSession,
    ) -> std::io::Result<SavedSession> {
        let mut sensitive_values = session.sensitive_user_input_provenance.snapshot();
        crate::runtime_threads::collect_sensitive_user_input_values(
            &session.messages,
            &mut sensitive_values,
        );
        let projected_artifact_sizes =
            Self::reproject_owned_artifact_files(session, &sensitive_values)?;
        let mut projected =
            crate::runtime_threads::redacted_serializable_clone(session, &sensitive_values)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        projected.messages = crate::runtime_threads::redacted_durable_history_clone(
            &session.messages,
            &sensitive_values,
        );
        for artifact in &mut projected.artifacts {
            let owner = if artifact.session_id.is_empty() {
                session.metadata.id.as_str()
            } else {
                artifact.session_id.as_str()
            };
            if let Some(byte_size) =
                projected_artifact_sizes.get(&(owner.to_string(), artifact.storage_path.clone()))
            {
                artifact.byte_size = *byte_size;
            }
        }
        Ok(projected)
    }

    /// Serialize a privacy-projected saved session for managed or explicit
    /// persistence. Callers must attach any process-local provenance first.
    pub(crate) fn serialize_public_session_pretty(
        session: &SavedSession,
    ) -> std::io::Result<Vec<u8>> {
        let projected = Self::project_saved_session_for_persistence(session)?;
        serde_json::to_vec_pretty(&projected)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    }

    fn validated_session_id<'a>(&self, id: &'a str) -> std::io::Result<&'a str> {
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
        Ok(trimmed)
    }

    fn validated_session_path(&self, id: &str) -> std::io::Result<PathBuf> {
        let trimmed = self.validated_session_id(id)?;
        Ok(self.sessions_dir.join(format!("{trimmed}.json")))
    }

    fn checkpoints_dir(&self) -> PathBuf {
        self.sessions_dir.join("checkpoints")
    }

    fn validated_checkpoint_path(&self, session_id: &str) -> std::io::Result<PathBuf> {
        let trimmed = self.validated_session_id(session_id)?;
        // Reserved file names inside `checkpoints/` must never collide with a
        // per-session checkpoint file.
        if format!("{trimmed}.json") == LEGACY_CHECKPOINT_FILE
            || format!("{trimmed}.json") == OFFLINE_QUEUE_FILE
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Session id '{trimmed}' collides with a reserved checkpoint file"),
            ));
        }
        Ok(self.checkpoints_dir().join(format!("{trimmed}.json")))
    }

    /// Create a new `SessionManager` with the specified sessions directory
    pub fn new(sessions_dir: PathBuf) -> std::io::Result<Self> {
        let sessions_dir = normalize_managed_dir(sessions_dir)?;
        // Ensure the sessions directory exists
        fs::create_dir_all(&sessions_dir)?;
        Ok(Self { sessions_dir })
    }

    /// Create a `SessionManager` using the default location.
    pub fn default_location() -> std::io::Result<Self> {
        Self::new(default_sessions_dir()?)
    }

    /// Return the resolved sessions directory path.
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }

    /// Save a session to disk using atomic write (temp file + fsync + rename).
    pub fn save_session(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let path = self.validated_session_path(&session.metadata.id)?;

        self.archive_before_first_graph_write(session, &path)?;
        let content = Self::serialize_public_session_pretty(session)?;

        // Atomic write via write_atomic (NamedTempFile + fsync + persist)
        write_atomic(&path, &content)?;

        // Clean up old sessions if we have too many
        self.cleanup_old_sessions()?;

        Ok(path)
    }

    /// Save a crash-recovery checkpoint for in-flight turns.
    ///
    /// Checkpoints are keyed per session (`checkpoints/<session_id>.json`) so
    /// concurrent sessions never overwrite each other's crash-recovery state.
    pub fn save_checkpoint(&self, session: &SavedSession) -> std::io::Result<PathBuf> {
        let path = self.validated_checkpoint_path(&session.metadata.id)?;
        let session_path = self.validated_session_path(&session.metadata.id)?;
        self.archive_before_first_graph_write(session, &session_path)?;
        fs::create_dir_all(self.checkpoints_dir())?;
        let content = Self::serialize_public_session_pretty(session)?;
        write_atomic(&path, &content)?;
        Ok(path)
    }

    /// Preserve the exact pre-import session once, before the first graph-
    /// bearing session or checkpoint write can replace it.
    fn archive_before_first_graph_write(
        &self,
        session: &SavedSession,
        source: &Path,
    ) -> std::io::Result<()> {
        let writes_graph = session
            .work_state
            .as_ref()
            .and_then(|state| state.graph.as_ref())
            .is_some_and(|graph| !graph.is_empty());
        if !writes_graph || !source.exists() {
            return Ok(());
        }
        let bytes = fs::read(source)?;
        let saved = serde_json::from_slice::<SavedSession>(&bytes)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let already_graph_backed = saved
            .work_state
            .as_ref()
            .and_then(|state| state.graph.as_ref())
            .is_some_and(|graph| !graph.is_empty());
        if already_graph_backed {
            return Ok(());
        }
        let archive_dir = self.sessions_dir.join(WORK_GRAPH_IMPORT_ARCHIVE_DIR);
        fs::create_dir_all(&archive_dir)?;
        let archive =
            archive_dir.join(source.file_name().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "invalid session path")
            })?);
        if !archive.exists() {
            let saved = saved;
            saved
                .sensitive_user_input_provenance
                .extend(session.sensitive_user_input_provenance.snapshot());
            let bytes = Self::serialize_public_session_pretty(&saved)?;
            write_atomic(&archive, &bytes)?;
        }
        Ok(())
    }

    fn read_checkpoint_file(&self, path: &Path) -> std::io::Result<Option<SavedSession>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        let mut session: SavedSession = serde_json::from_str(&content)
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
        session.system_prompt = strip_legacy_truncation_note(session.system_prompt);
        session.refresh_sensitive_user_input_provenance();
        Ok(Some(session))
    }

    /// Load a specific session's crash-recovery checkpoint if present.
    pub fn load_session_checkpoint(
        &self,
        session_id: &str,
    ) -> std::io::Result<Option<SavedSession>> {
        let path = self.validated_checkpoint_path(session_id)?;
        self.read_checkpoint_file(&path)
    }

    /// Load the legacy single-slot checkpoint (`checkpoints/latest.json`) if
    /// present. Compatibility read only — this release no longer writes it.
    pub fn load_legacy_checkpoint(&self) -> std::io::Result<Option<SavedSession>> {
        let path = self.checkpoints_dir().join(LEGACY_CHECKPOINT_FILE);
        self.read_checkpoint_file(&path)
    }

    /// Clear one session's crash-recovery checkpoint. Scoped: this can never
    /// remove another session's checkpoint file or the legacy slot.
    pub fn clear_session_checkpoint(&self, session_id: &str) -> std::io::Result<()> {
        let path = self.validated_checkpoint_path(session_id)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Remove the legacy single-slot checkpoint file.
    pub fn clear_legacy_checkpoint(&self) -> std::io::Result<()> {
        let path = self.checkpoints_dir().join(LEGACY_CHECKPOINT_FILE);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// Enumerate all crash-recovery checkpoint files (per-session files plus
    /// the legacy single slot), sorted most recently modified first. Only
    /// file metadata is read here; callers load content per candidate.
    pub fn list_checkpoints(&self) -> std::io::Result<Vec<CheckpointRef>> {
        let dir = self.checkpoints_dir();
        let mut refs = Vec::new();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(refs),
            Err(err) => return Err(err),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let source = if name == LEGACY_CHECKPOINT_FILE {
                CheckpointSource::Legacy
            } else if name == OFFLINE_QUEUE_FILE {
                continue;
            } else {
                let session_id = name.trim_end_matches(".json").to_string();
                if self.validated_checkpoint_path(&session_id).is_err() {
                    continue;
                }
                CheckpointSource::Session(session_id)
            };
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            refs.push(CheckpointRef {
                source,
                path,
                modified,
            });
        }
        refs.sort_by_key(|r| std::cmp::Reverse(r.modified));
        Ok(refs)
    }

    /// Migrate a session recovered from the legacy single-slot checkpoint to
    /// a per-session checkpoint file. Never overwrites an existing
    /// per-session file and leaves the legacy file in place (older binaries
    /// still read it; the legacy writer is already gone). Returns whether a
    /// file was written.
    pub fn write_session_checkpoint_if_absent(
        &self,
        session: &SavedSession,
    ) -> std::io::Result<bool> {
        let path = self.validated_checkpoint_path(&session.metadata.id)?;
        if path.exists() {
            return Ok(false);
        }
        self.save_checkpoint(session)?;
        Ok(true)
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
        let mut session: SavedSession = serde_json::from_str(&content)
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

        session.system_prompt = strip_legacy_truncation_note(session.system_prompt);

        let repair = crate::tool_history_repair::repair_tool_call_pairs(&mut session.messages);
        if !repair.is_empty() {
            session.metadata.message_count = session.messages.len();
            tracing::warn!(
                session_id = %session.metadata.id,
                repaired_call_ids = ?repair.repaired_call_ids,
                duplicate_result_ids = ?repair.duplicate_result_ids,
                orphan_result_ids = ?repair.orphan_result_ids,
                "repaired persisted tool call/result history"
            );
        }

        session.refresh_sensitive_user_input_provenance();

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

    /// Clean up old sessions to stay within `MAX_SESSIONS` limit.
    pub fn cleanup_old_sessions(&self) -> std::io::Result<()> {
        self.cleanup_old_sessions_keeping(None)
    }

    /// As [`Self::cleanup_old_sessions`], but never deletes `keep` — the
    /// session being resumed at boot. Without this, a background cleanup that
    /// races session restore can prune the just-resumed session when 50+
    /// newer records exist (its `updated_at` is not bumped until first save).
    pub fn cleanup_old_sessions_keeping(&self, keep: Option<&str>) -> std::io::Result<()> {
        let sessions = self.list_sessions()?;

        if sessions.len() > MAX_SESSIONS {
            for session in sessions.iter().skip(MAX_SESSIONS) {
                if keep.is_some_and(|id| id == session.id) {
                    continue;
                }
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
    /// Crash-recovery safety: skips the per-session checkpoint files
    /// (`checkpoints/<session_id>.json`), the legacy single-slot
    /// checkpoint (`checkpoints/latest.json`), and any file under `checkpoints/`
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
        self.prune_sessions_older_than_keeping(max_age, None)
    }

    /// As [`Self::prune_sessions_older_than`], but never deletes `keep` — the
    /// active session. A just-resumed session's `updated_at` is stale until
    /// its first post-resume save, so an age prune could otherwise delete the
    /// live session out from under the TUI.
    pub fn prune_sessions_older_than_keeping(
        &self,
        max_age: std::time::Duration,
        keep: Option<&str>,
    ) -> std::io::Result<usize> {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_age).unwrap_or(chrono::Duration::days(365 * 10));
        let sessions = self.list_sessions()?;
        let mut pruned = 0usize;
        for session in sessions {
            if keep.is_some_and(|id| id == session.id) {
                continue;
            }
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
        let git_entry = current.join(".git");
        if git_entry.exists() {
            return is_git_metadata_entry(&git_entry).then_some(current);
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

/// Resolve the default session directory path.
///
/// v0.8.44: prefers `~/.codewhale/sessions`, falls back to
/// `~/.deepseek/sessions` for existing installs. Uses the write-path resolver
/// so the first access relocates any legacy `~/.deepseek/sessions` into
/// `~/.codewhale/sessions` when the primary directory is missing (#3240).
/// If an older build already created an empty primary sessions directory, copy
/// missing legacy entries into it without overwriting newer CodeWhale data.
pub fn default_sessions_dir() -> std::io::Result<PathBuf> {
    let dir = codewhale_config::ensure_state_dir("sessions")
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))?;
    match merge_missing_legacy_session_entries(&dir) {
        Ok(0) => {}
        Ok(count) => {
            tracing::info!(
                target: "session::migration",
                "Copied {count} missing legacy session entries into {}",
                dir.display()
            );
        }
        Err(err) => {
            tracing::warn!(
                target: "session::migration",
                "Could not copy legacy sessions into {}: {err}",
                dir.display()
            );
        }
    }
    Ok(dir)
}

fn merge_missing_legacy_session_entries(primary: &Path) -> io::Result<usize> {
    if codewhale_home_is_explicit() {
        return Ok(0);
    }

    let legacy = codewhale_config::legacy_deepseek_home()
        .map_err(|e| io::Error::new(io::ErrorKind::NotFound, e.to_string()))?
        .join("sessions");
    if !legacy.is_dir() || paths_equivalent(primary, &legacy) {
        return Ok(0);
    }

    copy_missing_dir_entries(&legacy, primary)
}

fn codewhale_home_is_explicit() -> bool {
    std::env::var("CODEWHALE_HOME")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

fn copy_missing_dir_entries(src: &Path, dst: &Path) -> io::Result<usize> {
    fs::create_dir_all(dst)?;
    let mut copied = 0;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());

        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if entry.file_name() == std::ffi::OsStr::new("checkpoints") || target.exists() {
                continue;
            }
            copied += copy_missing_dir_entries(&source, &target)?;
        } else if file_type.is_file() {
            copied += usize::from(copy_file_create_new(&source, &target)?);
        }
    }
    Ok(copied)
}

fn copy_file_create_new(src: &Path, dst: &Path) -> io::Result<bool> {
    let mut source = fs::File::open(src)?;
    let mut target = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dst)
    {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(false),
        Err(err) => return Err(err),
    };
    if let Err(err) = io::copy(&mut source, &mut target) {
        let _ = fs::remove_file(dst);
        return Err(err);
    }
    Ok(true)
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

    let sensitive_user_input_provenance =
        crate::runtime_threads::SensitiveUserInputProvenance::default();
    let mut sensitive_values = std::collections::HashSet::new();
    crate::runtime_threads::collect_sensitive_user_input_values(messages, &mut sensitive_values);
    sensitive_user_input_provenance.extend(sensitive_values);

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
            model_provider: default_model_provider(),
            model_provider_id: None,
            workspace: workspace.to_path_buf(),
            mode: mode.map(str::to_string),
            cost: SessionCostSnapshot::default(),
            parent_session_id: None,
            forked_from_message_count: None,
            cumulative_turn_secs: 0,
        },
        messages: messages.to_vec(),
        system_prompt: system_prompt_to_string(system_prompt),
        context_references: Vec::new(),
        artifacts: Vec::new(),
        work_state: None,
        last_auto_route: None,
        sensitive_user_input_provenance,
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
    session.messages.clear();
    session.messages.extend_from_slice(messages);
    let mut sensitive_values = session.sensitive_user_input_provenance.snapshot();
    crate::runtime_threads::collect_sensitive_user_input_values(messages, &mut sensitive_values);
    session
        .sensitive_user_input_provenance
        .extend(sensitive_values);
    session.metadata.updated_at = Utc::now();
    session.metadata.message_count = messages.len();
    session.metadata.total_tokens = total_tokens;
    session.system_prompt = system_prompt_to_string(system_prompt);
    session
}

/// Strip a stale `[Session note]` block that was written by the old
/// 500-message cap. Only removes notes that contain the specific
/// "older messages were dropped" phrase — ordinary user-added
/// `[Session note]` prompts are left untouched.
fn strip_legacy_truncation_note(system_prompt: Option<String>) -> Option<String> {
    let sp = system_prompt?;
    let Some(trimmed) = sp.strip_prefix("[Session note]\n") else {
        return Some(sp);
    };
    // Only strip if this is the known cap_messages note.
    if !trimmed.contains("older messages were dropped") {
        return Some(sp);
    }
    // The note block ends with "\n\n---\n\n" (7 chars) followed by the real prompt.
    trimmed
        .find("\n\n---\n\n")
        .map(|pos| trimmed[pos + 7..].to_string())
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
    let updated = format_session_updated_at(&meta.updated_at, &age);
    let truncated_title = truncate_title(extract_title(&meta.title), 40);
    let fork_label = if meta.parent_session_id.is_some() {
        " | fork"
    } else {
        ""
    };

    format!(
        "{} | {} | {} msgs{} | {}",
        truncate_id(&meta.id),
        truncated_title,
        meta.message_count,
        fork_label,
        updated
    )
}

pub(crate) fn format_session_updated_at(dt: &DateTime<Utc>, age: &str) -> String {
    format!("{} ({age})", dt.format("%Y-%m-%d %H:%M UTC"))
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
    use crate::tools::plan::StepStatus;
    use crate::tui::history::{HistoryCell, ToolCell, history_cells_from_message};
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
                model_provider: "deepseek".to_string(),
                model_provider_id: None,
                workspace: workspace.to_path_buf(),
                mode: None,
                cost: SessionCostSnapshot::default(),
                parent_session_id: None,
                forked_from_message_count: None,
                cumulative_turn_secs: 0,
            },
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
            work_state: None,
            last_auto_route: None,
            sensitive_user_input_provenance: Default::default(),
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
                model_provider: "deepseek".to_string(),
                model_provider_id: None,
                workspace: workspace.to_path_buf(),
                mode: Some("yolo".to_string()),
                cost: SessionCostSnapshot::default(),
                parent_session_id: None,
                forked_from_message_count: None,
                cumulative_turn_secs: 0,
            },
            system_prompt: None,
            context_references: Vec::new(),
            artifacts: Vec::new(),
            work_state: None,
            last_auto_route: None,
            sensitive_user_input_provenance: Default::default(),
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
    fn load_session_repairs_dangling_tool_call_with_visible_receipt() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![Message {
            role: "assistant".to_string(),
            content: vec![ContentBlock::ToolUse {
                id: "call-crashed".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "README.md"}),
                caller: None,
            }],
        }];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 0, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).expect("save");

        let loaded = manager.load_session(&session_id).expect("load");

        assert_eq!(loaded.metadata.message_count, loaded.messages.len());
        assert!(loaded.messages.iter().any(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error: Some(true),
                        ..
                    } if tool_use_id == "call-crashed" && content.contains("crashed_and_repaired")
                )
            })
        }));
        assert!(loaded.messages.iter().any(|message| {
            message.role == "assistant"
                && message.content.iter().any(|block| {
                    matches!(
                        block,
                        ContentBlock::Text { text, .. }
                            if text.contains("[tool_history_repair]")
                    )
                })
        }));
    }

    #[test]
    fn save_and_load_session_preserves_rich_update_plan_tool_payload() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![
            make_test_message("user", "plan this carefully"),
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "plan-1".to_string(),
                    name: "update_plan".to_string(),
                    input: serde_json::json!({
                        "objective": "Make Plan mode reviewable",
                        "sources_used": ["gh issue view 2691"],
                        "critical_files": ["crates/tui/src/tools/plan.rs"],
                        "constraints": ["Preserve legacy update_plan payloads"],
                        "verification_plan": "Run focused plan tests",
                        "handoff_packet": "Next agent should inspect replay",
                        "plan": [
                            { "step": "render replay card", "status": "completed" }
                        ]
                    }),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "plan-1".to_string(),
                    content: "Plan updated".to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let session = create_saved_session(&messages, "deepseek-v4-flash", tmp.path(), 42, None);
        let session_id = session.metadata.id.clone();

        manager.save_session(&session).expect("save");
        let loaded = manager.load_session(&session_id).expect("load");

        assert_eq!(loaded.messages.len(), 3);
        let cells = history_cells_from_message(&loaded.messages[1]);
        let Some(HistoryCell::Tool(ToolCell::PlanUpdate(cell))) = cells.first() else {
            panic!("expected loaded update_plan to replay as a PlanUpdate cell");
        };
        assert_eq!(
            cell.snapshot.objective.as_deref(),
            Some("Make Plan mode reviewable")
        );
        assert_eq!(
            cell.snapshot.critical_files,
            vec!["crates/tui/src/tools/plan.rs"]
        );
        assert_eq!(cell.snapshot.items[0].status, StepStatus::Completed);
    }

    #[test]
    fn save_session_preserves_large_tool_outputs_for_cache_fidelity() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let raw = "RAW_SESSION_SENTINEL\n".repeat(2_000);
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call-big".to_string(),
                    name: "exec_shell".to_string(),
                    input: serde_json::json!({"command": "cargo test -p codewhale-tui"}),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-big".to_string(),
                    content: raw.clone(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call-big".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-big".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw.len() as u64,
            preview: "checking crate ... error[E0425]".to_string(),
            storage_path: PathBuf::from("artifacts/art_call-big.txt"),
        });

        let path = manager.save_session(&session).expect("save");
        let persisted_json = fs::read_to_string(path).expect("read persisted session");
        // Raw output is preserved in-session so resume can hit the LLM cache.
        assert!(persisted_json.contains("RAW_SESSION_SENTINEL"));

        let loaded = manager.load_session(&session.metadata.id).expect("load");
        let ContentBlock::ToolResult { content, .. } = &loaded.messages[1].content[0] else {
            panic!("expected loaded tool result");
        };
        // Loaded session retains the original output for cache fidelity.
        assert!(content.contains("RAW_SESSION_SENTINEL"));
        assert!(!content.contains("[TOOL_OUTPUT_RECEIPT]"));
    }

    #[test]
    fn load_session_preserves_legacy_large_tool_outputs_for_cache_fidelity() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let raw = "RAW_LEGACY_RESUME_SENTINEL\n".repeat(2_000);
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "call-legacy".to_string(),
                    name: "exec_shell".to_string(),
                    input: serde_json::json!({"command": "cargo check"}),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "call-legacy".to_string(),
                    content: raw.clone(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 100, None);
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_call-legacy".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-legacy".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw.len() as u64,
            preview: "cargo check output".to_string(),
            storage_path: PathBuf::from("artifacts/art_call-legacy.txt"),
        });
        let path = manager
            .validated_session_path(&session.metadata.id)
            .expect("path");
        fs::write(
            &path,
            serde_json::to_string_pretty(&session).expect("serialize legacy session"),
        )
        .expect("write legacy raw session");
        assert!(
            fs::read_to_string(&path)
                .expect("read legacy raw")
                .contains("RAW_LEGACY_RESUME_SENTINEL")
        );

        let loaded = manager.load_session(&session.metadata.id).expect("load");
        let ContentBlock::ToolResult { content, .. } = &loaded.messages[1].content[0] else {
            panic!("expected loaded tool result");
        };
        // Loaded session preserves original output so resume can hit the LLM cache.
        assert!(content.contains("RAW_LEGACY_RESUME_SENTINEL"));
        assert!(!content.contains("[TOOL_OUTPUT_RECEIPT]"));
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
    fn default_manager_copies_legacy_sessions_when_primary_already_exists() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");

        let primary_sessions = home.join(".codewhale").join("sessions");
        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::create_dir_all(legacy_sessions.join("checkpoints")).expect("legacy checkpoints");
        fs::write(
            legacy_sessions.join("checkpoints").join("latest.json"),
            "{}",
        )
        .expect("legacy checkpoint");

        let mut legacy_session = create_saved_session(
            &[make_test_message("user", "find my old session")],
            "test-model",
            tmp.path(),
            100,
            None,
        );
        legacy_session.metadata.id = "legacy-visible".to_string();
        legacy_session.metadata.title = "session from legacy home".to_string();
        fs::write(
            legacy_sessions.join("legacy-visible.json"),
            serde_json::to_string_pretty(&legacy_session).expect("serialize legacy session"),
        )
        .expect("write legacy session");

        let manager = SessionManager::default_location().expect("default manager");
        assert_eq!(manager.sessions_dir(), primary_sessions.as_path());
        assert!(primary_sessions.join("legacy-visible.json").exists());
        assert!(!primary_sessions.join("checkpoints").exists());
        assert!(legacy_sessions.join("legacy-visible.json").exists());

        let sessions = manager.list_sessions().expect("list");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "legacy-visible");
    }

    #[test]
    fn legacy_session_copy_never_overwrites_primary_session() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home = crate::test_support::EnvVarGuard::remove("CODEWHALE_HOME");

        let primary_sessions = home.join(".codewhale").join("sessions");
        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&primary_sessions).expect("primary sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");

        let primary_path = primary_sessions.join("same-id.json");
        fs::write(&primary_path, "primary data wins").expect("write primary session");
        fs::write(
            legacy_sessions.join("same-id.json"),
            "legacy data must not overwrite",
        )
        .expect("write legacy session");

        let dir = default_sessions_dir().expect("default session dir");
        assert_eq!(dir, primary_sessions);
        assert_eq!(
            fs::read_to_string(primary_path).expect("read primary session"),
            "primary data wins"
        );
    }

    #[test]
    fn explicit_codewhale_home_disables_legacy_session_copy() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("home");
        let explicit_home = tmp.path().join("explicit-codewhale");
        let _home = crate::test_support::EnvVarGuard::set("HOME", &home);
        let _codewhale_home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &explicit_home);

        let legacy_sessions = home.join(".deepseek").join("sessions");
        fs::create_dir_all(&legacy_sessions).expect("legacy sessions");
        fs::write(legacy_sessions.join("legacy-visible.json"), "{}").expect("write legacy session");

        let dir = default_sessions_dir().expect("default session dir");
        assert_eq!(dir, explicit_home.join("sessions"));
        assert!(!dir.join("legacy-visible.json").exists());
    }

    #[test]
    fn latest_session_for_workspace_ignores_newer_other_directory() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let workspace_a = tmp.path().join("aa").join("aaa");
        let workspace_b = tmp.path().join("bb").join("bbb");
        fs::create_dir_all(&workspace_a).expect("mkdir workspace a");
        fs::create_dir_all(&workspace_b).expect("mkdir workspace b");
        fs::create_dir_all(tmp.path().join(".git")).expect("mkdir invalid git boundary");

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
    fn format_session_line_includes_absolute_updated_timestamp() {
        let mut session = create_saved_session(
            &[make_test_message("user", "Find Friday work")],
            "test-model",
            Path::new("/tmp/project"),
            100,
            None,
        );
        session.metadata.updated_at = DateTime::parse_from_rfc3339("2026-06-01T12:34:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);

        let line = format_session_line(&session.metadata);

        assert!(
            line.contains("2026-06-01 12:34 UTC"),
            "session list should include an absolute timestamp, got {line:?}"
        );
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
    fn save_load_round_trip_preserves_all_messages_for_cache_fidelity() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        // Covers the old 500-message cap boundary and well beyond.
        for count in [0, 1, 500, 501, 600, 1000] {
            let original: Vec<_> = (0..count)
                .map(|i| {
                    make_test_message(
                        if i % 2 == 0 { "user" } else { "assistant" },
                        &format!("round-trip message {i}"),
                    )
                })
                .collect();

            let session = create_saved_session(&original, "test-model", tmp.path(), 0, None);
            manager.save_session(&session).expect("save");
            let loaded = manager.load_session(&session.metadata.id).expect("load");

            assert_eq!(
                loaded.messages.len(),
                count,
                "count preserved for count={count}"
            );
            assert_eq!(
                loaded.messages, original,
                "every message byte-identical after round-trip for count={count}"
            );
        }
    }

    #[test]
    fn test_checkpoint_round_trip_and_clear() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![make_test_message("user", "checkpoint me")];
        let mut session = create_saved_session(&messages, "test-model", tmp.path(), 12, None);
        session.work_state = Some(SessionWorkState {
            todos: crate::tools::todo::TodoListSnapshot {
                items: vec![crate::tools::todo::TodoItem {
                    id: 1,
                    content: "verify checkpoint durability".to_string(),
                    status: crate::tools::todo::TodoStatus::InProgress,
                }],
                completion_pct: 0,
                in_progress_id: Some(1),
            },
            ..SessionWorkState::default()
        });

        let path = manager.save_checkpoint(&session).expect("save checkpoint");
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some(format!("{}.json", session.metadata.id).as_str()),
            "checkpoint file must be keyed by session id"
        );
        let loaded = manager
            .load_session_checkpoint(&session.metadata.id)
            .expect("load checkpoint")
            .expect("checkpoint exists");
        assert_eq!(loaded.metadata.id, session.metadata.id);
        assert_eq!(loaded.messages, session.messages);
        assert_eq!(
            loaded.work_state, session.work_state,
            "work state must survive the checkpoint round trip"
        );

        manager
            .clear_session_checkpoint(&session.metadata.id)
            .expect("clear checkpoint");
        assert!(
            manager
                .load_session_checkpoint(&session.metadata.id)
                .expect("load checkpoint")
                .is_none()
        );
    }

    #[test]
    fn session_and_checkpoint_files_project_typed_modal_answers_recursively() {
        const SECRET: &str = "482915";
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "input-call".to_string(),
                    name: "request_user_input".to_string(),
                    input: serde_json::json!({
                        "questions": [{
                            "header": "PIN",
                            "id": "pin",
                            "question": "Enter PIN",
                            "options": [{"label": "Skip", "description": "Skip"}]
                        }]
                    }),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "input-call".to_string(),
                    content: serde_json::json!({
                        "answers": [{"id": "pin", "label": "Other", "value": SECRET}]
                    })
                    .to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "echo-call".to_string(),
                    name: "echo".to_string(),
                    input: serde_json::json!({
                        format!("key-{SECRET}"): {"value": format!("echo {SECRET}")}
                    }),
                    caller: None,
                }],
            },
        ];
        let session = create_saved_session(&messages, "test-model", tmp.path(), 12, None);

        let session_path = manager.save_session(&session).expect("save session");
        let checkpoint_path = manager.save_checkpoint(&session).expect("save checkpoint");
        for path in [session_path, checkpoint_path] {
            let raw = fs::read_to_string(&path).expect("read projected file");
            assert!(
                !raw.contains(SECRET),
                "secret persisted in {}",
                path.display()
            );
            let decoded: SavedSession =
                serde_json::from_str(&raw).expect("fixed SavedSession schema remains valid");
            assert_eq!(decoded.schema_version, CURRENT_SESSION_SCHEMA_VERSION);
            assert_eq!(decoded.messages.len(), 3);
            assert!(matches!(
                &decoded.messages[0].content[0],
                ContentBlock::ToolUse { id, name, .. }
                    if id == "input-call" && name == "request_user_input"
            ));
            assert!(matches!(
                &decoded.messages[1].content[0],
                ContentBlock::ToolResult { content, .. }
                    if content == "User input submitted"
            ));
        }
    }

    #[test]
    fn saved_session_projects_non_message_fields_and_owned_artifact_bytes() {
        const SECRET: &str = "owned-artifact-secret-482915";
        const SHORT: &str = "running";

        struct ArtifactRootReset(Option<PathBuf>);
        impl Drop for ArtifactRootReset {
            fn drop(&mut self) {
                crate::artifacts::set_test_artifact_sessions_root(self.0.take());
            }
        }

        let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let _artifact_root = ArtifactRootReset(crate::artifacts::set_test_artifact_sessions_root(
            Some(sessions_dir.clone()),
        ));
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let system_prompt =
            crate::models::SystemPrompt::Text(format!("system echoed {SECRET} {SHORT}"));
        let mut session = create_saved_session(
            &[make_test_message("user", "safe source message")],
            SHORT,
            tmp.path(),
            12,
            Some(&system_prompt),
        );
        session.metadata.title = format!("title echoed {SECRET} {SHORT}");
        session.work_state = Some(SessionWorkState {
            plan: crate::tools::plan::PlanSnapshot {
                objective: Some(format!("objective {SECRET} {SHORT}")),
                sources_used: vec![format!("source {SECRET} {SHORT}")],
                items: vec![crate::tools::plan::PlanItemArg {
                    step: format!("step {SECRET} {SHORT}"),
                    status: crate::tools::plan::StepStatus::Pending,
                }],
                ..crate::tools::plan::PlanSnapshot::default()
            },
            ..SessionWorkState::default()
        });
        let storage_path = PathBuf::from("artifacts/art_privacy.txt");
        let artifact_path = sessions_dir.join(&session.metadata.id).join(&storage_path);
        fs::create_dir_all(artifact_path.parent().expect("artifact parent")).expect("artifact dir");
        let raw_artifact = format!("tool output {SECRET} {SHORT}\n");
        fs::write(&artifact_path, &raw_artifact).expect("raw artifact");
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_privacy".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-privacy".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: raw_artifact.len() as u64,
            preview: format!("preview {SECRET} {SHORT}"),
            storage_path: storage_path.clone(),
        });
        session
            .sensitive_user_input_provenance
            .extend([SECRET.to_string(), SHORT.to_string()]);

        let saved_path = manager
            .save_session(&session)
            .expect("save projected session");
        let raw_session = fs::read_to_string(saved_path).expect("saved session bytes");
        assert!(!raw_session.contains(SECRET), "{raw_session}");
        let saved_artifact = fs::read_to_string(&artifact_path).expect("projected artifact");
        assert!(!saved_artifact.contains(SECRET), "{saved_artifact}");
        assert!(!saved_artifact.contains(SHORT), "{saved_artifact}");

        let loaded = manager
            .load_session(&session.metadata.id)
            .expect("load projected session");
        assert_eq!(loaded.metadata.model, SHORT, "model identity is structural");
        assert!(!loaded.metadata.title.contains(SECRET) && !loaded.metadata.title.contains(SHORT));
        assert!(
            loaded
                .system_prompt
                .as_deref()
                .is_some_and(|prompt| !prompt.contains(SECRET) && !prompt.contains(SHORT))
        );
        let plan = &loaded.work_state.as_ref().expect("Work state").plan;
        assert!(
            plan.sources_used
                .iter()
                .all(|source| !source.contains(SECRET) && !source.contains(SHORT))
        );
        assert!(loaded.artifacts[0].preview.find(SECRET).is_none());
        assert_eq!(
            loaded.artifacts[0].byte_size,
            saved_artifact.len() as u64,
            "metadata follows rewritten artifact bytes"
        );
    }

    #[test]
    fn session_and_checkpoint_persistence_preserve_owned_binary_artifacts() {
        const SECRET: &str = "482915";

        struct ArtifactRootReset(Option<PathBuf>);
        impl Drop for ArtifactRootReset {
            fn drop(&mut self) {
                crate::artifacts::set_test_artifact_sessions_root(self.0.take());
            }
        }

        let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let _artifact_root = ArtifactRootReset(crate::artifacts::set_test_artifact_sessions_root(
            Some(sessions_dir.clone()),
        ));
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let mut session = create_saved_session(&[], "model", tmp.path(), 0, None);
        let storage_path = PathBuf::from("artifacts/art_binary.png");
        let artifact_path = sessions_dir.join(&session.metadata.id).join(&storage_path);
        fs::create_dir_all(artifact_path.parent().expect("artifact parent")).expect("artifact dir");
        let png_signature = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        fs::write(&artifact_path, png_signature).expect("binary artifact");
        session.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "art_binary".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: session.metadata.id.clone(),
            tool_call_id: "call-binary".to_string(),
            tool_name: "fetch_url".to_string(),
            created_at: Utc::now(),
            byte_size: png_signature.len() as u64,
            preview: format!("fetched image {SECRET}"),
            storage_path: storage_path.clone(),
        });
        session
            .sensitive_user_input_provenance
            .extend([SECRET.to_string()]);

        manager
            .save_session(&session)
            .expect("binary-safe session save");
        manager
            .save_checkpoint(&session)
            .expect("binary-safe checkpoint save");

        assert_eq!(
            fs::read(&artifact_path).expect("binary bytes"),
            png_signature
        );
        let loaded = manager
            .load_session(&session.metadata.id)
            .expect("load binary session");
        assert_eq!(loaded.artifacts[0].storage_path, storage_path);
        assert_eq!(loaded.artifacts[0].byte_size, png_signature.len() as u64);
        assert!(!loaded.artifacts[0].preview.contains(SECRET));
    }

    #[test]
    fn fork_persistence_never_rewrites_parent_owned_artifact_bytes() {
        const SECRET: &str = "482915";

        struct ArtifactRootReset(Option<PathBuf>);
        impl Drop for ArtifactRootReset {
            fn drop(&mut self) {
                crate::artifacts::set_test_artifact_sessions_root(self.0.take());
            }
        }

        let _artifact_guard = crate::artifacts::TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let tmp = tempdir().expect("tempdir");
        let sessions_dir = tmp.path().join("sessions");
        let _artifact_root = ArtifactRootReset(crate::artifacts::set_test_artifact_sessions_root(
            Some(sessions_dir.clone()),
        ));
        let manager = SessionManager::new(sessions_dir.clone()).expect("new");
        let parent_id = "parent7";
        let storage_path = PathBuf::from("artifacts/shared.txt");
        let parent_path = sessions_dir.join(parent_id).join(&storage_path);
        fs::create_dir_all(parent_path.parent().expect("parent artifact dir"))
            .expect("parent artifact dir");
        let parent_bytes = format!("parent-owned output {SECRET}\n");
        fs::write(&parent_path, parent_bytes.as_bytes()).expect("parent artifact");

        let mut fork = create_saved_session(&[], "model", tmp.path(), 0, None);
        fork.metadata.id = "child-fork".to_string();
        fork.metadata.parent_session_id = Some(parent_id.to_string());
        fork.artifacts.push(crate::artifacts::ArtifactRecord {
            id: "shared".to_string(),
            kind: crate::artifacts::ArtifactKind::ToolOutput,
            session_id: parent_id.to_string(),
            tool_call_id: "call-parent".to_string(),
            tool_name: "exec_shell".to_string(),
            created_at: Utc::now(),
            byte_size: parent_bytes.len() as u64,
            preview: format!("parent preview {SECRET}"),
            storage_path: storage_path.clone(),
        });
        fork.sensitive_user_input_provenance
            .extend([SECRET.to_string(), "7".to_string()]);

        manager.save_session(&fork).expect("save fork");
        manager.save_checkpoint(&fork).expect("checkpoint fork");

        assert_eq!(
            fs::read(&parent_path).expect("parent artifact after fork save"),
            parent_bytes.as_bytes(),
            "a fork may reference but never mutate its parent's artifact"
        );
        let loaded = manager.load_session(&fork.metadata.id).expect("load fork");
        assert_eq!(loaded.artifacts[0].session_id, parent_id);
        assert_eq!(loaded.artifacts[0].storage_path, storage_path);
        assert!(!loaded.artifacts[0].preview.contains(SECRET));
    }

    #[test]
    fn loaded_modal_provenance_survives_source_compaction_without_serializing_registry() {
        const SECRET: &str = "7";
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let source_messages = vec![
            Message {
                role: "assistant".to_string(),
                content: vec![ContentBlock::ToolUse {
                    id: "input-call".to_string(),
                    name: "request_user_input".to_string(),
                    input: serde_json::json!({
                        "questions": [{
                            "header": "PIN",
                            "id": "pin",
                            "question": "Enter PIN",
                            "options": [{"label": "Skip", "description": "Skip"}]
                        }]
                    }),
                    caller: None,
                }],
            },
            Message {
                role: "user".to_string(),
                content: vec![ContentBlock::ToolResult {
                    tool_use_id: "input-call".to_string(),
                    content: serde_json::json!({
                        "answers": [{"id": "pin", "label": "Other", "value": SECRET}]
                    })
                    .to_string(),
                    is_error: None,
                    content_blocks: None,
                }],
            },
        ];
        let source = create_saved_session(&source_messages, "test-model", tmp.path(), 1, None);
        let path = manager
            .validated_session_path(&source.metadata.id)
            .expect("session path");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&source).expect("legacy raw session"),
        )
        .expect("write legacy raw session");

        let loaded = manager
            .load_session(&source.metadata.id)
            .expect("load raw legacy session");
        assert!(
            loaded
                .sensitive_user_input_provenance
                .snapshot()
                .contains(SECRET),
            "load must reconstruct volatile provenance while the source remains"
        );

        let compacted = [make_test_message(
            "assistant",
            "The submitted PIN was 7; continuing.",
        )];
        let compacted = update_session(loaded, &compacted, 2, None);
        let session_path = manager.save_session(&compacted).expect("save compacted");
        let checkpoint_path = manager
            .save_checkpoint(&compacted)
            .expect("checkpoint compacted");

        for persisted_path in [session_path, checkpoint_path] {
            let raw = fs::read_to_string(&persisted_path).expect("read projected persistence");
            assert!(
                !raw.contains("PIN was 7"),
                "compacted echo leaked in {}",
                persisted_path.display()
            );
            assert!(
                !raw.contains("sensitive_user_input_provenance"),
                "volatile provenance registry was serialized in {}",
                persisted_path.display()
            );
            serde_json::from_str::<SavedSession>(&raw)
                .expect("projected persistence keeps the fixed SavedSession schema");
        }
    }

    #[test]
    fn work_graph_archive_projects_compacted_echo_with_incoming_live_provenance() {
        const SECRET: &str = "482915";
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut legacy = create_saved_session(
            &[make_test_message(
                "assistant",
                "Compacted history still echoed 482915.",
            )],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let path = manager
            .validated_session_path(&legacy.metadata.id)
            .expect("session path");
        fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy).expect("legacy raw session"),
        )
        .expect("write legacy raw session");

        let plan = crate::tools::plan::PlanSnapshot {
            items: vec![crate::tools::plan::PlanItemArg {
                step: "Import".to_string(),
                status: crate::tools::plan::StepStatus::Pending,
            }],
            ..crate::tools::plan::PlanSnapshot::default()
        };
        let todos = crate::tools::todo::TodoListSnapshot::default();
        let graph = crate::work_graph::import_legacy(&legacy.metadata.id, &plan, &todos)
            .expect("import graph");
        legacy.work_state = Some(SessionWorkState {
            graph: Some(graph),
            todos,
            plan,
        });
        legacy
            .sensitive_user_input_provenance
            .extend([SECRET.to_string()]);

        manager.save_session(&legacy).expect("first graph write");
        let archive = manager
            .sessions_dir
            .join(WORK_GRAPH_IMPORT_ARCHIVE_DIR)
            .join(path.file_name().expect("session filename"));
        let raw = fs::read_to_string(&archive).expect("projected archive");
        assert!(!raw.contains(SECRET), "archive leaked compacted echo");
        assert!(
            !raw.contains("sensitive_user_input_provenance"),
            "archive serialized volatile provenance registry"
        );
        serde_json::from_str::<SavedSession>(&raw)
            .expect("projected archive keeps the fixed SavedSession schema");
    }

    #[test]
    fn graph_backed_work_state_remains_readable_by_legacy_shape() {
        #[derive(serde::Deserialize)]
        struct LegacyWorkState {
            #[serde(default)]
            todos: crate::tools::todo::TodoListSnapshot,
            #[serde(default)]
            plan: crate::tools::plan::PlanSnapshot,
        }

        let fixture = include_bytes!("../tests/fixtures/work_graph_session_v1_reader.json");
        let current: SavedSession = serde_json::from_slice(fixture).expect("current reader");
        let state = current.work_state.expect("fixture Work state");
        let legacy: LegacyWorkState = serde_json::from_value(
            serde_json::from_slice::<serde_json::Value>(fixture)
                .expect("fixture JSON")["work_state"]
                .clone(),
        )
        .expect("v1 reader ignores graph");
        assert_eq!(legacy.todos, state.todos);
        assert_eq!(legacy.plan, state.plan);
        let graph = state.graph.expect("fixture graph");
        crate::work_graph::validate(&graph).expect("valid fixture graph");
        assert_eq!(crate::work_graph::project_todos(&graph), state.todos);
        assert_eq!(crate::work_graph::project_plan(&graph), state.plan);
    }

    #[test]
    fn first_graph_write_archives_exact_legacy_session_once() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "archive before import")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let plan = crate::tools::plan::PlanSnapshot {
            items: vec![crate::tools::plan::PlanItemArg {
                step: "Import".to_string(),
                status: crate::tools::plan::StepStatus::Pending,
            }],
            ..crate::tools::plan::PlanSnapshot::default()
        };
        let todos = crate::tools::todo::TodoListSnapshot::default();
        session.work_state = Some(SessionWorkState {
            graph: None,
            todos: todos.clone(),
            plan: plan.clone(),
        });
        let path = manager.save_session(&session).expect("save legacy session");
        let legacy_bytes = fs::read(&path).expect("read legacy bytes");

        let graph = crate::work_graph::import_legacy(&session.metadata.id, &plan, &todos)
            .expect("import graph");
        session.work_state = Some(SessionWorkState {
            graph: Some(graph),
            todos,
            plan,
        });
        manager.save_session(&session).expect("first graph write");
        let archive = manager
            .sessions_dir
            .join(WORK_GRAPH_IMPORT_ARCHIVE_DIR)
            .join(path.file_name().expect("session filename"));
        assert_eq!(fs::read(&archive).expect("archive exists"), legacy_bytes);

        session.metadata.title = "later graph write".to_string();
        manager.save_session(&session).expect("second graph write");
        assert_eq!(
            fs::read(&archive).expect("archive still exists"),
            legacy_bytes,
            "later graph writes must not replace the pre-import receipt"
        );
    }

    #[test]
    fn checkpoints_are_independent_per_session() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let first = create_saved_session(
            &[make_test_message("user", "session one")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        let second = create_saved_session(
            &[make_test_message("user", "session two")],
            "test-model",
            tmp.path(),
            0,
            None,
        );

        manager.save_checkpoint(&first).expect("save first");
        manager.save_checkpoint(&second).expect("save second");
        manager
            .clear_session_checkpoint(&first.metadata.id)
            .expect("clear first");

        assert!(
            manager
                .load_session_checkpoint(&first.metadata.id)
                .expect("load first")
                .is_none(),
            "clearing one session must remove only that session's file"
        );
        let survivor = manager
            .load_session_checkpoint(&second.metadata.id)
            .expect("load second")
            .expect("second checkpoint survives");
        assert_eq!(survivor.metadata.id, second.metadata.id);
    }

    #[test]
    fn list_checkpoints_includes_legacy_slot_and_skips_offline_queue() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let session = create_saved_session(
            &[make_test_message("user", "list me")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        manager.save_checkpoint(&session).expect("save checkpoint");
        let checkpoints = tmp.path().join("sessions").join("checkpoints");
        fs::write(checkpoints.join("latest.json"), "{}").expect("write legacy slot");
        fs::write(checkpoints.join("offline_queue.json"), "{}").expect("write offline queue");

        let refs = manager.list_checkpoints().expect("list checkpoints");
        assert_eq!(refs.len(), 2, "offline queue must not be a candidate");
        assert!(
            refs.iter()
                .any(|r| r.source == CheckpointSource::Session(session.metadata.id.clone()))
        );
        assert!(refs.iter().any(|r| r.source == CheckpointSource::Legacy));
    }

    #[test]
    fn legacy_migration_never_overwrites_existing_per_session_checkpoint() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let mut session = create_saved_session(
            &[make_test_message("user", "original")],
            "test-model",
            tmp.path(),
            0,
            None,
        );
        manager.save_checkpoint(&session).expect("save checkpoint");

        session.messages = vec![make_test_message("user", "stale legacy copy")];
        let written = manager
            .write_session_checkpoint_if_absent(&session)
            .expect("migration attempt");
        assert!(!written, "migration must not overwrite an existing file");
        let loaded = manager
            .load_session_checkpoint(&session.metadata.id)
            .expect("load")
            .expect("checkpoint exists");
        assert_eq!(
            loaded.messages,
            vec![make_test_message("user", "original")],
            "existing per-session checkpoint content must be preserved"
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
                skill_provenance: None,
            }],
            draft: Some(QueuedSessionMessage {
                display: "draft message".to_string(),
                skill_instruction: None,
                skill_provenance: None,
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
                skill_provenance: None,
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

        let err = manager
            .load_legacy_checkpoint()
            .expect_err("should reject schema");
        assert!(err.to_string().contains("newer than supported"));

        // The same guard applies to per-session checkpoint files.
        fs::rename(&path, checkpoints.join("sid.json")).expect("rename to per-session file");
        let err = manager
            .load_session_checkpoint("sid")
            .expect_err("should reject schema");
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
                    {{ "role": "user", "content": [ {{ "Text": {{ "text": {big_text:?} }} }} ] }}
                ]
            }}"#
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
        assert!(session.last_auto_route.is_none());
        assert!(session.metadata.parent_session_id.is_none());
        assert!(session.metadata.forked_from_message_count.is_none());
    }

    #[test]
    fn fork_lineage_metadata_round_trips_and_formats() {
        let tmp = tempdir().expect("tempdir");
        let manager = SessionManager::new(tmp.path().join("sessions")).expect("new");
        let parent = create_saved_session(
            &[
                make_test_message("user", "try approach A"),
                make_test_message("assistant", "A looks viable"),
            ],
            "deepseek-v4-pro",
            Path::new("/tmp"),
            42,
            None,
        );
        let mut forked = create_saved_session(
            &parent.messages,
            &parent.metadata.model,
            &parent.metadata.workspace,
            parent.metadata.total_tokens,
            None,
        );
        forked.metadata.mark_forked_from(&parent.metadata);

        manager.save_session(&forked).expect("save fork");
        let loaded = manager
            .load_session(&forked.metadata.id)
            .expect("load fork");

        assert_eq!(
            loaded.metadata.parent_session_id.as_deref(),
            Some(parent.metadata.id.as_str())
        );
        assert_eq!(loaded.metadata.forked_from_message_count, Some(2));
        let line = format_session_line(&loaded.metadata);
        assert!(line.contains("fork"));
        assert!(!line.contains(parent.metadata.id.as_str()));
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
