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
//! to decide whether to re-dispatch. For operate-mode goals the only terminal
//! stops are a verified completion, a blocked report, or an exhausted
//! token/time budget (#5052); a configurable safety backstop
//! (`[goal] max_continuations`) still halts a pathological loop that never
//! emits a terminal signal, and logs when it fires.

/// Default safety backstop on automatic cross-turn continuation passes for one
/// goal run (#5052).
///
/// This is deliberately generous: the completion gate and token/time budgets
/// are the real terminal stops, and the backstop only exists to halt a
/// pathological loop that never emits a terminal signal. Override with
/// `[goal] max_continuations` in config.toml; `0` disables the backstop
/// entirely so only budget/terminal stops end the run.
pub const DEFAULT_MAX_GOAL_CONTINUATIONS: u32 = 100;

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
/// for that resource; the continuation backstop (`max_continuations`) still
/// applies unless configured to `0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalBudget {
    pub token_budget: Option<u64>,
    pub time_budget_seconds: Option<u64>,
    /// Safety backstop on automatic continuation passes (#5052). `0` disables
    /// the backstop: only terminal status, user control, or token/time budget
    /// exhaustion stop the run.
    pub max_continuations: u32,
}

impl GoalBudget {
    /// No token or time cap. Terminal status, user control, and the default
    /// continuation backstop still stop the run.
    #[allow(dead_code)]
    pub const fn unbounded() -> Self {
        Self {
            token_budget: None,
            time_budget_seconds: None,
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
        }
    }

    /// A token budget only — the loop runs until the model is done or the
    /// token budget is exhausted.
    #[allow(dead_code)]
    pub const fn with_token_budget(token_budget: u64) -> Self {
        Self {
            token_budget: Some(token_budget),
            time_budget_seconds: None,
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
        }
    }

    /// Override the continuation backstop (`0` = unlimited-with-budget-stops).
    #[allow(dead_code)]
    #[must_use]
    pub const fn with_max_continuations(mut self, max_continuations: u32) -> Self {
        self.max_continuations = max_continuations;
        self
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
/// 3. The configurable continuation backstop stops a pathological loop
///    (skipped entirely when configured to `0`).
/// 4. Otherwise continue — the loop runs to the completion gate, not to a
///    fixed pass count (#5052).
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
    // orchestration subsystem. `0` disables it — budget/terminal stops only.
    if budget.max_continuations > 0 && progress.continuations >= budget.max_continuations {
        tracing::warn!(
            continuations = progress.continuations,
            max_continuations = budget.max_continuations,
            "goal continuation backstop fired: no terminal signal after the configured \
             continuation limit ([goal] max_continuations)"
        );
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
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn active_under_continuation_limit_without_budget_continues() {
        let progress = GoalProgress {
            continuations: DEFAULT_MAX_GOAL_CONTINUATIONS - 1,
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
            continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Stop(StopReason::ContinuationLimit)
        );
    }

    #[test]
    fn operate_goal_continues_past_ten_when_budget_remains() {
        // #5052 regression: the old hardcoded cap of 10 must not be a terminal
        // stop. With budget remaining and no terminal signal, pass 10, 11, and
        // far beyond keep continuing under the default backstop.
        for continuations in [10, 11, DEFAULT_MAX_GOAL_CONTINUATIONS - 1] {
            let progress = GoalProgress {
                tokens_used: 5_000,
                time_used_seconds: 300,
                continuations,
                ..GoalProgress::default()
            };
            let budget = GoalBudget::with_token_budget(1_000_000);
            assert_eq!(
                decide_continuation(GoalRunStatus::Active, progress, budget),
                ContinuationDecision::Continue,
                "pass {continuations} must continue toward the completion gate",
            );
        }
    }

    #[test]
    fn configured_backstop_halts_pathological_loop() {
        let backstop = 25;
        let progress = GoalProgress {
            continuations: backstop,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::unbounded().with_max_continuations(backstop);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::ContinuationLimit)
        );
    }

    #[test]
    fn zero_backstop_is_unlimited_but_budget_still_stops() {
        // 0 = unlimited-with-budget-stops: no continuation count ends the run…
        let progress = GoalProgress {
            continuations: 10_000,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::unbounded().with_max_continuations(0);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue
        );

        // …but an exhausted token budget still does.
        let progress = GoalProgress {
            tokens_used: 1_000,
            continuations: 10_000,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::with_token_budget(1_000).with_max_continuations(0);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::TokenBudget)
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
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
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
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Completed, progress, budget),
            ContinuationDecision::Stop(StopReason::Completed)
        );
    }
}
