//! Operations submitted by the UI to the core engine.
//!
//! These operations flow from the TUI to the engine via a channel,
//! allowing the UI to remain responsive while the engine processes requests.

use crate::compaction::CompactionConfig;
use crate::models::{Message, SystemPrompt};
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use std::path::PathBuf;

/// Prefix used for tool-call ids created by local composer shell shortcuts.
pub const USER_SHELL_TOOL_ID_PREFIX: &str = "user_shell_";

/// Snapshot of session state for saving to disk.
/// Returned by `Op::GetSessionSnapshot` via a oneshot channel.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub messages: Vec<Message>,
    pub total_tokens: u64,
    pub model: String,
    pub workspace: PathBuf,
    pub system_prompt: Option<SystemPrompt>,
    pub mode: String,
}

/// Operations that can be submitted to the engine.
#[derive(Debug, Clone)]
pub enum Op {
    /// Send a message to the AI
    SendMessage {
        content: String,
        mode: AppMode,
        model: String,
        goal_objective: Option<String>,
        /// Reasoning-effort tier: `"off" | "low" | "medium" | "high" | "max"`.
        /// `None` lets the provider apply its default.
        reasoning_effort: Option<String>,
        /// True when the user selected auto thinking, even though the UI sends
        /// a concrete per-turn value to the model API.
        reasoning_effort_auto: bool,
        /// True when the user selected auto model routing.
        auto_model: bool,
        allow_shell: bool,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
        translation_enabled: bool,
        show_thinking: bool,
        /// Tool restriction from custom slash command frontmatter.
        /// `None` means the current turn may use the normal tool set.
        allowed_tools: Option<Vec<String>>,
        /// Hook executor for control-plane hooks.
        /// `ToolCallBefore` hooks may deny a tool call with exit code 2.
        hook_executor: Option<std::sync::Arc<crate::hooks::HookExecutor>>,
        verbosity: Option<String>,
    },

    /// Execute a user-submitted composer shell command (`! <command>`) without
    /// sending a model turn. This still routes through `exec_shell`, approval,
    /// sandbox, and command-safety handling.
    RunShellCommand {
        command: String,
        mode: AppMode,
        trust_mode: bool,
        auto_approve: bool,
        approval_mode: ApprovalMode,
    },

    /// Cancel the current request
    #[allow(dead_code)]
    CancelRequest,

    /// Approve a tool call that requires permission
    #[allow(dead_code)]
    ApproveToolCall { id: String },

    /// Deny a tool call that requires permission
    #[allow(dead_code)]
    DenyToolCall { id: String },

    /// Spawn a sub-agent
    #[allow(dead_code)]
    SpawnSubAgent { prompt: String },

    /// List current sub-agents and their status
    ListSubAgents,

    /// Change the operating mode
    #[allow(dead_code)]
    ChangeMode { mode: AppMode },

    /// Update the model being used and refresh stable prompt context.
    #[allow(dead_code)]
    SetModel { model: String, mode: AppMode },

    /// Update auto-compaction settings
    SetCompaction { config: CompactionConfig },

    /// Update the SSE idle timeout used for subsequent streamed turns.
    SetStreamChunkTimeout { timeout_secs: u64 },

    /// Sync engine session state (used for resume/load)
    SyncSession {
        session_id: Option<String>,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        system_prompt_override: bool,
        model: String,
        workspace: PathBuf,
    },

    /// Run context compaction immediately.
    CompactContext,

    /// Get a snapshot of the current session state (messages, tokens, etc.)
    /// for saving to disk. Returns the result via the oneshot sender so
    /// the caller doesn't have to compete with the SSE event stream.
    GetSessionSnapshot {
        tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<SessionSnapshot>>>>,
    },

    /// Run agent-driven context purging.
    PurgeContext,

    /// Edit the last user message: remove the last user+assistant exchange
    /// from the session, then re-send with the new content.
    #[allow(dead_code)]
    EditLastTurn { new_message: String },

    /// Shutdown the engine
    Shutdown,
}
