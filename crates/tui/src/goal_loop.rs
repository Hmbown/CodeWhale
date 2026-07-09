//! Goal loop orchestrator — the persistent-objective control layer (#3215, and
//! its lineage #891 / #1976 / #2058 / #2029).
//!
//! This is the **Workflow goal layer**: the decision core that turns a one-shot
//! `/goal` into a persistent work loop. Given the durable goal status, the
//! accumulated usage (from the per-goal accounting wired in `crates/state`
//! `record_thread_goal_usage`), and a budget, it decides whether to **continue**
//! (re-dispatch another worker turn toward the objective) or **stop** with a
//! terminal status. It is the orchestrator in the Workflow≈ultracode mapping —
//! the loop that fans work out to workers (`worker_profile`) and verifies before
//! committing.
//!
//! Scope: **decision logic + types**. The engine (`core/engine.rs`) reads the
//! `SharedGoalState` snapshot after each turn and calls `decide_continuation`
//! to decide whether to re-dispatch. There is **no continuation cap** — a goal
//! runs until the model self-reports complete/blocked, the user pauses or
//! clears, or an optional token/time budget is exhausted. This matches how a
//! persistent objective should feel: "until done," not "until N turns."

/// Terminal or active state of a persistent goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalRunStatus {
    /// Still working toward the objective.
    Active,
    /// The objective was achieved (the model self-reported done and, ideally, a
    /// verifier confirmed — see `GoalGate`).
    Completed,
    /// The model reported it is blocked and needs the user.
    #[allow(dead_code)]
    Blocked,
}

/// Why the loop stopped, for a terminal decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Objective achieved.
    Completed,
    /// Model reported blocked.
    #[allow(dead_code)]
    Blocked,
    /// Token budget exhausted.
    TokenBudget,
    /// Wall-clock budget exhausted.
    TimeBudget,
    /// Continuation circuit-breaker tripped (too many continuations without a
    /// terminal signal). Retained for API completeness; the current loop has no
    /// continuation cap, so this variant is not constructed by
    /// `decide_continuation`.
    #[allow(dead_code)]
    ContinuationLimit,
}

/// Accumulated, durable progress for a goal run. Mirrors the fields wired by
/// `crates/state` `record_thread_goal_usage` (tokens_used / time_used_seconds)
/// plus a continuation counter the loop maintains.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoalProgress {
    pub tokens_used: u64,
    pub time_used_seconds: u64,
    pub continuations: u32,
}

/// The bound on a goal run. `None` fields mean unbounded. There is **no
/// continuation cap** — the loop runs until the model self-reports
/// complete/blocked, the user pauses/clears, or an optional budget is
/// exhausted. This is deliberate: a goal is "until done," not "until N turns."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalBudget {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<u64>,
}

impl GoalBudget {
    /// Fully unbounded — no token or time cap. The only stops are a terminal
    /// model status (complete/blocked) or an explicit user pause/clear.
    #[allow(dead_code)]
    pub const fn unbounded() -> Self {
        Self {
            token_budget: None,
            time_budget_seconds: None,
        }
    }

    /// A token budget only — the loop runs until the model is done or the
    /// token budget is exhausted.
    #[allow(dead_code)]
    pub const fn with_token_budget(token_budget: u64) -> Self {
        Self {
            token_budget: Some(token_budget),
            time_budget_seconds: None,
        }
    }
}

/// The decision the loop makes after each worker turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationDecision {
    /// Re-dispatch another turn toward the objective.
    Continue,
    /// Stop; the goal run is terminal.
    Stop(StopReason),
}

/// Decide whether a persistent goal run should continue after a turn.
///
/// Precedence (most authoritative first):
/// 1. A terminal model status (Completed / Blocked) ends the run.
/// 2. An optional token or time budget, if exhausted, ends the run.
/// 3. Otherwise continue.
///
/// There is **no continuation cap**. A goal runs until the model reports
/// done/blocked, the user pauses or clears, or an optional budget is spent.
#[must_use]
pub fn decide_continuation(
    status: GoalRunStatus,
    progress: GoalProgress,
    budget: GoalBudget,
) -> ContinuationDecision {
    // 1. Terminal model signal wins.
    match status {
        GoalRunStatus::Completed => return ContinuationDecision::Stop(StopReason::Completed),
        GoalRunStatus::Blocked => return ContinuationDecision::Stop(StopReason::Blocked),
        GoalRunStatus::Active => {}
    }

    // 2. Optional budget. No continuation cap — "until done."
    if let Some(tokens) = budget.token_budget
        && progress.tokens_used >= tokens
    {
        return ContinuationDecision::Stop(StopReason::TokenBudget);
    }
    if let Some(secs) = budget.time_budget_seconds
        && progress.time_used_seconds >= secs
    {
        return ContinuationDecision::Stop(StopReason::TimeBudget);
    }

    // 3. Keep going.
    ContinuationDecision::Continue
}

/// Whether a stop reason represents success (Completed) vs. an early/forced exit.
/// Useful for the UI/status projection (#2666 token/time visibility).
#[must_use]
#[allow(dead_code)]
pub fn is_success(reason: StopReason) -> bool {
    matches!(reason, StopReason::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_status_stops_with_success() {
        let d = decide_continuation(
            GoalRunStatus::Completed,
            GoalProgress::default(),
            GoalBudget::unbounded(),
        );
        assert_eq!(d, ContinuationDecision::Stop(StopReason::Completed));
        assert!(is_success(StopReason::Completed));
    }

    #[test]
    fn blocked_status_stops_without_success() {
        let d = decide_continuation(
            GoalRunStatus::Blocked,
            GoalProgress::default(),
            GoalBudget::unbounded(),
        );
        assert_eq!(d, ContinuationDecision::Stop(StopReason::Blocked));
        assert!(!is_success(StopReason::Blocked));
    }

    #[test]
    fn active_under_budget_continues() {
        let progress = GoalProgress {
            tokens_used: 10,
            time_used_seconds: 5,
            continuations: 2,
        };
        let budget = GoalBudget {
            token_budget: Some(1000),
            time_budget_seconds: Some(600),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn active_with_no_budget_continues_indefinitely() {
        // No continuation cap: a high continuation count with no token/time
        // budget must still Continue. The loop is "until done," not "until N."
        let progress = GoalProgress {
            continuations: 1_000_000,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn token_budget_exhaustion_stops() {
        let progress = GoalProgress {
            tokens_used: 1000,
            continuations: 1,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::with_token_budget(1000);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TokenBudget)
        );
    }

    #[test]
    fn time_budget_exhaustion_stops() {
        let progress = GoalProgress {
            time_used_seconds: 601,
            continuations: 1,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: None,
            time_budget_seconds: Some(600),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TimeBudget)
        );
    }

    #[test]
    fn terminal_status_outranks_remaining_budget() {
        // Completed wins even if there is plenty of budget left.
        let progress = GoalProgress::default();
        let budget = GoalBudget {
            token_budget: Some(1_000_000),
            time_budget_seconds: Some(86_400),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Completed, progress, budget),
            ContinuationDecision::Stop(StopReason::Completed)
        );
    }
}
