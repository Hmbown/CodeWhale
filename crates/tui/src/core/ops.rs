//! Operations submitted by the UI to the core engine.
//!
//! These operations flow from the TUI to the engine via a channel,
//! allowing the UI to remain responsive while the engine processes requests.

use crate::compaction::CompactionConfig;
use crate::models::{Message, SystemPrompt};
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use std::path::PathBuf;

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

    /// Update the model being used
    #[allow(dead_code)]
    SetModel { model: String },

    /// Update auto-compaction settings
    SetCompaction { config: CompactionConfig },

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

    /// Edit the last user message: remove the last user+assistant exchange
    /// from the session, then re-send with the new content.
    #[allow(dead_code)]
    EditLastTurn { new_message: String },

    /// Shutdown the engine
    Shutdown,
}
