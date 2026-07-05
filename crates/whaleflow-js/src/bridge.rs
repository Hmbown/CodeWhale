//! `Send`-only channel types bridging the single-threaded QuickJS VM to the
//! multi-thread engine (design §3.2). Only `Send` data crosses these
//! channels — strings and `serde_json` values, never a `Ctx` or `'js` value.

use codewhale_tools::ToolCallSource;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

/// One `await task({...})` call. The driver spawns the child through the
/// existing fire-and-forget engine, then resolves `reply` from its
/// completion pump keyed by `agent_id` (design §3.1–3.4). Spawn rejections
/// (budget / admission / depth) resolve `reply` with `Err` immediately and
/// surface as a JS exception.
#[derive(Debug)]
pub struct SpawnRequest {
    /// The raw `task()` options object (JSON). Field mapping follows design
    /// §3.3 / `parse_spawn_request` semantics; the driver normalizes JS
    /// spellings (`description`, `subagentType`, `allowedTools`).
    pub input: Value,
    pub reply: oneshot::Sender<Result<TaskCompletion, String>>,
}

/// Resolution payload for a completed `task()`: the FULL result text read
/// from `SubAgentManager::get_result`, not the truncated mailbox summary.
#[derive(Debug, Clone)]
pub struct TaskCompletion {
    pub agent_id: String,
    pub text: String,
}

/// One `await tools.<name>(args)` programmatic tool call (design §8).
#[derive(Debug)]
pub struct ToolCallRequest {
    pub name: String,
    pub input: Value,
    /// Always [`ToolCallSource::JsRepl`] for calls originating in the VM;
    /// carried for audit/telemetry on the driver side.
    pub source: ToolCallSource,
    pub reply: oneshot::Sender<Result<ToolCallOutcome, String>>,
}

/// Raw tool executor result. The VM decodes it for JS as
/// `metadata ?? JSON.parse(content) ?? String(content)`.
#[derive(Debug, Clone)]
pub struct ToolCallOutcome {
    pub success: bool,
    pub content: String,
    pub metadata: Option<Value>,
}

/// Live read of the active budget scope for `budget.spent()` /
/// `budget.remaining()` (design §5.2).
#[derive(Debug)]
pub struct BudgetQuery {
    pub reply: oneshot::Sender<BudgetSnapshot>,
}

/// Snapshot of the orchestrator's budget scope. `total`/`remaining` are
/// `None` when no budget is configured (unlimited); `remaining` already
/// subtracts outstanding spawn reservations (design §5.3).
#[derive(Debug, Clone, Copy, Default)]
pub struct BudgetSnapshot {
    pub total: Option<u64>,
    pub spent: u64,
    pub remaining: Option<u64>,
}

/// The sender half handed to the VM. Everything here is `Send + Sync` and
/// cheap to clone; the receivers live in the tui-side driver pump.
#[derive(Clone)]
pub struct HostChannels {
    pub spawn_tx: mpsc::Sender<SpawnRequest>,
    pub tool_tx: mpsc::Sender<ToolCallRequest>,
    pub budget_tx: mpsc::Sender<BudgetQuery>,
    pub log_tx: mpsc::UnboundedSender<String>,
}
