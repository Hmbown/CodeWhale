//! WhaleFlow — declarative multi-agent workflow orchestration for CodeWhale.
//!
//! WhaleFlow lets DeepSeek orchestrate sub-agent swarms at scale using a
//! declarative JSON config. The model writes a workflow plan with phases,
//! tasks, and dependencies; the scheduler fans out sub-agents, pipes results
//! between dependent tasks, and returns an integrated structured result.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────┐     ┌──────────────────┐     ┌─────────────────┐
//! │ WorkflowConfig│ ──▶ │    Scheduler     │ ──▶ │  AgentSpawner   │
//! │   (JSON)      │     │ (topo sort, fan) │     │   (trait)       │
//! └─────────────┘     └──────────────────┘     └────────┬────────┘
//!                                                       │
//!                                               ┌───────▼────────┐
//!                                               │  TUI crate      │
//!                                               │ (SubAgentRuntime)│
//!                                               └────────────────┘
//! ```
//!
//! The `whaleflow` crate is pure orchestration logic. It has zero
//! dependencies on the TUI, network, or filesystem. The embedding
//! application provides a concrete [`AgentSpawner`] implementation.

pub mod config;
pub mod result;
pub mod scheduler;
pub mod spawner;
pub mod worktree;
pub mod tool;

pub use config::{
    Conflict, ConflictKind, FailurePolicy, IsolationMode, Phase, Task, TaskMode, WorkflowConfig,
};
pub use result::{TaskStatus, WorkflowResult, WorkflowStatus};
pub use scheduler::Scheduler;
pub use spawner::{AgentResult, AgentSpawner, SpawnError};
pub use worktree::WorktreeManager;
