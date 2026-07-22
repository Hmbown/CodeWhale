//! The driver seam between the sandboxed VM and the subagent engine.
//!
//! The QuickJS VM lives on a dedicated thread and its `'js` values can never
//! cross an `.await` onto another thread, so everything that leaves the VM is
//! plain `Send` data: a [`TaskRequest`] goes out, a [`TaskCompletion`] comes
//! back over a oneshot. The [`WorkflowDriver`] trait is the host-side contract
//! the tui wiring implements over `SubAgentManager` (spawn is fire-and-forget
//! there; the driver's completion pump resolves the oneshot from the mailbox
//! `Completed` signal keyed by `agent_id`, then reads the full untruncated
//! text via `get_result`). Tests implement it with
//! [`crate::testing::FakeDriver`].
//!
//! Budget ownership: token accounting and the §5.3 reservation semantics live
//! entirely on the driver side (the manager's budget scopes). The VM only
//! reads [`BudgetSnapshot`]s — it performs a fast-fail `spent >= total` check
//! before spawning and exposes the numbers to JS as `budget.*`, but it never
//! reserves or debits tokens itself. A driver that admits a spawn is the
//! authority; its rejection surfaces as a JS throw on that `task()` call.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::error::DriverError;

/// One `task()` invocation, fully resolved and validated on the VM side.
///
/// Field semantics mirror the `agent` tool's spawn options.
///
/// Step identity is fleet `role` (preferred) and/or `profile` (#4177). Both
/// tokens are normalized (trimmed + lowercased) with the same rule as
/// `crates/workflow` leaf profiles. Roster membership is resolved by the
/// driver (tui) at spawn time — this crate never sees the saved Fleet roster.
/// Provider/model remain optional overrides, not required identity fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    /// The child prompt (JS `prompt`, falling back to `description`; required).
    pub description: String,
    /// Subagent type (JS `subagentType` or `type`); `None` lets the driver
    /// apply its default (`general`).
    pub subagent_type: Option<String>,
    /// Fleet role name (JS `role`), e.g. `scout` / `implementer` (#4177).
    pub role: Option<String>,
    /// Fleet profile token, normalized (trimmed, lowercased) and validated.
    /// Explicit profile wins over role mapping at spawn time.
    pub profile: Option<String>,
    /// Explicit model override; always wins over `model_strength`.
    pub model: Option<String>,
    /// Relative model strength (`same`/`faster`, plus driver-side aliases).
    pub model_strength: Option<String>,
    /// Reasoning effort (`inherit`/`off`/`low`/`medium`/`high`/`max`).
    pub thinking: Option<String>,
    /// Run the child in a fresh git worktree for parallel edits.
    pub worktree: bool,
    /// Explicit child mutation authority. A write-capable value remains
    /// fail-closed unless at least one bounded scope below is declared.
    #[serde(default)]
    pub write_authority: Option<String>,
    /// Repo-relative directory trees the child expects to mutate.
    #[serde(default)]
    pub write_roots: Vec<String>,
    /// Repo-relative exact files the child expects to mutate.
    #[serde(default)]
    pub exact_files: Vec<String>,
    /// Named shared contracts owned by this child while active.
    #[serde(default)]
    pub coordination_contracts: Vec<String>,
    /// Bounded prerequisite facts relevant to this child.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// Bounded observable checks for child completion.
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// Explicit tool allowlist; required by the driver for `custom` roles.
    pub allowed_tools: Option<Vec<String>>,
    /// Per-call spawn-depth override (driver clamps to its ceiling).
    pub max_depth: Option<u32>,
    /// Explicit token budget: forks an isolated pool on the driver side.
    /// Omit it so the child inherits (and debits) the shared run pool.
    pub token_budget: Option<u64>,
    /// Maximum model turns for this child (driver clamps to its ceiling).
    pub max_steps: Option<u32>,
    /// Hard wall-clock limit for this child in seconds.
    pub wall_time_secs: Option<u64>,
    /// JSON schema the reply must satisfy; validated in the VM after the
    /// driver returns the raw text (see [`crate`] docs for decode rules).
    pub response_schema: Option<serde_json::Value>,
    /// Short human label for progress surfaces.
    pub label: Option<String>,
    /// Phase name this task belongs to, for progress grouping.
    pub phase: Option<String>,
}

/// Terminal outcome of one spawned task, delivered over the completion
/// oneshot. Everything except `Completed` becomes a JS throw on the awaiting
/// `task()` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskCompletion {
    /// The child finished; `text` is the full, untruncated result.
    Completed { text: String },
    /// The child failed (error result, timeout, ...).
    Failed { message: String },
    /// The child was cancelled (cascade or explicit).
    Cancelled,
    /// The child's budget scope drained mid-flight.
    BudgetExhausted { message: String },
}

/// A successfully admitted spawn: the driver-assigned task id (the engine's
/// `agent_id`) plus the oneshot the driver resolves on completion.
///
/// Dropping the receiver must not wedge the driver; drivers should treat a
/// closed completion channel as "nobody is listening" and move on.
#[derive(Debug)]
pub struct SpawnedTask {
    /// Driver-assigned id, unique within the run (engine `agent_id`).
    pub task_id: String,
    /// Resolved exactly once with the terminal [`TaskCompletion`].
    pub completion: oneshot::Receiver<TaskCompletion>,
}

/// Live view of the run's shared token pool, owned by the driver.
///
/// `total == None` means no ceiling is configured; JS then sees
/// `budget.total === null` and `budget.remaining() === Infinity`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetSnapshot {
    /// Pool ceiling in tokens, if one is configured.
    pub total: Option<u64>,
    /// Tokens spent (plus driver-side reservations) against the pool.
    pub spent: u64,
}

impl BudgetSnapshot {
    /// Tokens left before the ceiling; `None` when the pool is unbounded.
    pub fn remaining(&self) -> Option<u64> {
        self.total.map(|total| total.saturating_sub(self.spent))
    }

    /// True once the pool has a ceiling and it is fully consumed.
    pub fn exhausted(&self) -> bool {
        matches!(self.total, Some(total) if self.spent >= total)
    }
}

/// Progress events emitted by the script (`log(..)` / `phase(..)`), delivered
/// to the driver synchronously and in script order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProgressEvent {
    /// `log(msg)` — a narrator line for the UI.
    Log {
        /// The stringified message.
        message: String,
    },
    /// `phase(title)` — the script entered a named phase.
    Phase {
        /// The phase title.
        title: String,
    },
    /// A completed child returned text that failed the caller's
    /// `responseSchema`. The VM emits this before throwing the validation
    /// error back into the script so host-side receipts can mark the leaf as
    /// failed instead of reporting a successful child beside a `null` result.
    TaskSchemaValidationFailed {
        /// Driver-assigned task id (engine `agent_id`).
        task_id: String,
        /// The validation error already surfaced to JS.
        message: String,
    },
}

/// Host-side executor for a Workflow run.
///
/// Implementations must be cheap to call from the VM thread: `spawn_task`
/// admits the task and returns immediately (fire-and-forget spawn — never
/// await the child inline), while `budget`, `progress`, and `cancel_all` are
/// synchronous. `cancel_all` must be idempotent; it is invoked when the
/// script errors, when the run future is dropped, and once more never hurts.
#[async_trait]
pub trait WorkflowDriver: Send + Sync {
    /// Admit and start one task. Errors surface as a JS throw on the
    /// corresponding `task()` call.
    async fn spawn_task(&self, request: TaskRequest) -> Result<SpawnedTask, DriverError>;

    /// Cancel every in-flight task belonging to this run. Idempotent.
    fn cancel_all(&self);

    /// Current snapshot of the run's shared token pool.
    fn budget(&self) -> BudgetSnapshot;

    /// Receive a script progress event (ordered, synchronous).
    fn progress(&self, event: ProgressEvent);
}

/// Normalize and validate a Fleet profile token: trim, lowercase, then apply
/// the same token rule as `crates/workflow`'s `validate_leaf_profile` —
/// non-empty, no whitespace, and none of `"`, `'`, `` ` ``, `=`.
pub fn normalize_profile(raw: &str) -> Result<String, String> {
    let normalized = raw.trim().to_lowercase();
    let invalid = normalized.is_empty()
        || normalized
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '"' | '\'' | '`' | '='));
    if invalid {
        return Err(format!(
            "invalid profile token {raw:?}: profiles must be non-empty and contain no whitespace, quotes, backticks, or '='"
        ));
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_profile_trims_and_lowercases() {
        assert_eq!(normalize_profile("  ALpha-1  ").unwrap(), "alpha-1");
    }

    #[test]
    fn normalize_profile_rejects_bad_tokens() {
        for bad in ["", "   ", "two words", "a=b", "a\"b", "a'b", "a`b"] {
            assert!(
                normalize_profile(bad).is_err(),
                "expected rejection: {bad:?}"
            );
        }
    }

    #[test]
    fn budget_snapshot_math() {
        let unbounded = BudgetSnapshot {
            total: None,
            spent: 10,
        };
        assert_eq!(unbounded.remaining(), None);
        assert!(!unbounded.exhausted());

        let pool = BudgetSnapshot {
            total: Some(100),
            spent: 40,
        };
        assert_eq!(pool.remaining(), Some(60));
        assert!(!pool.exhausted());

        let drained = BudgetSnapshot {
            total: Some(100),
            spent: 120,
        };
        assert_eq!(drained.remaining(), Some(0));
        assert!(drained.exhausted());
    }

    #[test]
    fn legacy_task_request_defaults_new_coordination_fields() {
        let legacy = serde_json::json!({
            "description": "inspect the candidate",
            "subagent_type": null,
            "role": "reviewer",
            "profile": null,
            "model": null,
            "model_strength": null,
            "thinking": null,
            "worktree": false,
            "allowed_tools": null,
            "max_depth": null,
            "token_budget": null,
            "max_steps": null,
            "wall_time_secs": null,
            "response_schema": null,
            "label": null,
            "phase": null
        });

        let request: TaskRequest = serde_json::from_value(legacy).unwrap();
        assert_eq!(request.write_authority, None);
        assert!(request.write_roots.is_empty());
        assert!(request.exact_files.is_empty());
        assert!(request.coordination_contracts.is_empty());
        assert!(request.dependencies.is_empty());
        assert!(request.acceptance.is_empty());
    }
}
