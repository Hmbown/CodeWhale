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
//! to decide whether to re-dispatch. A small cross-turn circuit breaker keeps
//! an unbounded goal from silently spending forever when the model never emits
//! a terminal signal; explicit token/time budgets still take precedence.

/// Fallback continuation circuit-breaker used when no explicit cap is
/// configured via [`GoalBudget::max_continuations`].
///
/// This matches the conservative run-cap used by the peer goal lifecycle while
/// avoiding its much larger classifier/strategist subsystem. In operate mode,
/// `[workflow] goal_continuation_cap` overrides this so that token/time budgets
/// become the primary resource limits instead of a fixed count.
pub const MAX_GOAL_CONTINUATIONS: u32 = 10;

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
    /// terminal signal).
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

/// The optional token/time bounds on a goal run. `None` fields mean unbounded
/// for that resource; the continuation circuit breaker applies unless
/// `max_continuations` overrides it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalBudget {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<u64>,
    /// Configurable continuation safety backstop.
    ///
    /// `None` falls back to [`MAX_GOAL_CONTINUATIONS`] (the conservative
    /// default). Set to `u32::MAX` for an effectively unlimited run where
    /// token/time budgets are the real resource limits (operate-mode default).
    /// Operators can lower this to any finite value.
    pub max_continuations: Option<u32>,
}

impl GoalBudget {
    /// No token, time, or explicit continuation cap. Terminal status, user
    /// control, and the fallback [`MAX_GOAL_CONTINUATIONS`] circuit breaker
    /// still stop the run.
    #[allow(dead_code)]
    pub const fn unbounded() -> Self {
        Self {
            token_budget: None,
            time_budget_seconds: None,
            max_continuations: None,
        }
    }

    /// A token budget only — the loop runs until the model is done or the
    /// token budget is exhausted.
    #[allow(dead_code)]
    pub const fn with_token_budget(token_budget: u64) -> Self {
        Self {
            token_budget: Some(token_budget),
            time_budget_seconds: None,
            max_continuations: None,
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
/// 3. The continuation circuit breaker stops a runaway loop: uses
///    `budget.max_continuations` when set, otherwise falls back to
///    [`MAX_GOAL_CONTINUATIONS`]. In operate mode, `max_continuations`
///    should be set high (e.g. `u32::MAX`) so that token/time budgets are
///    the real resource limits.
/// 4. Otherwise continue.
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

    // 2. Optional budget.
    if token_budget_exhausted(progress, budget) {
        return ContinuationDecision::Stop(StopReason::TokenBudget);
    }
    if let Some(secs) = budget.time_budget_seconds
        && progress.time_used_seconds >= secs
    {
        return ContinuationDecision::Stop(StopReason::TimeBudget);
    }

    // 3. Runaway-cost backstop. This deliberately uses the already-durable
    // continuation counter instead of adding verifier fingerprints or another
    // orchestration subsystem.
    let cap = budget.max_continuations.unwrap_or(MAX_GOAL_CONTINUATIONS);
    if progress.continuations >= cap {
        return ContinuationDecision::Stop(StopReason::ContinuationLimit);
    }

    // 4. Keep going.
    ContinuationDecision::Continue
}

/// Whether the durable token usage has reached the active goal's budget.
///
/// Kept as the shared terminal predicate so offline request inspection cannot
/// drift from the continuation gate that decides whether production may send
/// another goal turn.
#[must_use]
pub const fn token_budget_exhausted(progress: GoalProgress, budget: GoalBudget) -> bool {
    match budget.token_budget {
        Some(tokens) => progress.tokens_used >= tokens,
        None => false,
    }
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
            max_continuations: None,
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn active_under_continuation_limit_without_budget_continues() {
        let progress = GoalProgress {
            continuations: MAX_GOAL_CONTINUATIONS - 1,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn continuation_limit_stops_unbounded_run() {
        let progress = GoalProgress {
            continuations: MAX_GOAL_CONTINUATIONS,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Stop(StopReason::ContinuationLimit)
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
            max_continuations: None,
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
            max_continuations: None,
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Completed, progress, budget),
            ContinuationDecision::Stop(StopReason::Completed)
        );
    }

    #[test]
    fn custom_continuation_cap_stops_at_configured_limit() {
        // A cap of 50 should stop when continuations reach 50.
        let progress = GoalProgress {
            continuations: 50,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: None,
            time_budget_seconds: None,
            max_continuations: Some(50),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::ContinuationLimit)
        );
    }

    #[test]
    fn custom_continuation_cap_continues_below_limit() {
        // At 49 continuations with a cap of 50, the run should continue.
        let progress = GoalProgress {
            continuations: 49,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: None,
            time_budget_seconds: None,
            max_continuations: Some(50),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn max_u32_cap_lets_token_budget_be_the_real_limit() {
        // Operate-mode default: max_continuations = u32::MAX means the
        // continuation counter is never the stopping condition; only
        // token/time budgets (or terminal status) stop the run.
        let progress = GoalProgress {
            continuations: 1000,
            tokens_used: 1000,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: Some(1000),
            time_budget_seconds: None,
            max_continuations: Some(u32::MAX),
        };
        // Token budget is the stop here, not the continuation counter.
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TokenBudget)
        );
    }

    #[test]
    fn max_u32_cap_continues_past_old_default() {
        // With max_continuations = u32::MAX and no budget, a run at 100
        // continuations (far above the old 10 cap) should still continue.
        let progress = GoalProgress {
            continuations: 100,
            ..GoalProgress::default()
        };
        let budget = GoalBudget {
            token_budget: None,
            time_budget_seconds: None,
            max_continuations: Some(u32::MAX),
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }
}
