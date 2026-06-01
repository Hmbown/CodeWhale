//! Abstract agent-spawning interface.
//!
//! WhaleFlow orchestrates sub-agents without depending on any specific
//! harness or runtime. The [`AgentSpawner`] trait is the seam: the
//! scheduler calls `spawn()`, and the embedding application (e.g. the
//! CodeWhale TUI crate) provides the concrete implementation backed by
//! `SubAgentRuntime`.

use std::path::PathBuf;

use async_trait::async_trait;

/// Result of a single agent invocation.
#[derive(Debug, Clone)]
pub struct AgentResult {
    /// The task id from the workflow config.
    pub task_id: String,
    /// Whether the agent completed without error.
    pub success: bool,
    /// Human-readable summary of findings / actions taken.
    pub summary: String,
    /// Paths the agent read or modified.
    pub files_touched: Vec<String>,
    /// Raw output for piping to dependent tasks (may be large).
    pub raw_output: Option<String>,
    /// Total tokens consumed by this agent (prompt + completion).
    pub tokens_used: Option<u64>,
    /// Cost in USD for this agent's API usage.
    pub cost_usd: Option<f64>,
    /// Elapsed wall-clock time for this agent.
    pub elapsed_ms: Option<u64>,
    /// Last completed tool call for progress display.
    pub last_checkpoint: Option<String>,
}

/// Error conditions for agent spawning.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("agent spawn timeout: {0}")]
    Timeout(String),
    #[error("agent spawn failed: {0}")]
    SpawnFailed(String),
    #[error("agent spawn cancelled: {0}")]
    Cancelled(String),
    #[error("worktree setup failed: {0}")]
    WorktreeError(String),
    #[error("worktree cleanup failed: {0}")]
    CleanupError(String),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Abstract interface for spawning a single agent.
///
/// The embedding application (TUI crate) implements this trait using
/// the existing `SubAgentManager` / `SubAgentRuntime` infrastructure.
/// This keeps `crates/whaleflow` free of TUI dependencies.
#[async_trait]
pub trait AgentSpawner: Send + Sync {
    /// Spawn a single agent with the given task.
    ///
    /// If `cwd` is provided, the agent runs in that directory (used for
    /// worktree isolation). The spawner is responsible for creating and
    /// cleaning up the worktree if `isolation` is `Worktree`.
    ///
    /// `timeout_secs` caps total wall-clock time for this agent
    /// (including polling). `max_steps` maps to the sub-agent's
    /// `max_depth` to bound recursive tool calls.
    ///
    /// The spawner should handle model selection, tool gating, and
    /// session lifecycle. The scheduler only cares about the result.
    async fn spawn(
        &self,
        task_id: String,
        prompt: String,
        agent_type: Option<String>,
        cwd: Option<PathBuf>,
        timeout_secs: Option<u64>,
        max_steps: Option<u32>,
    ) -> Result<AgentResult, SpawnError>;
}
