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
//! stops are a verified completion, a blocked report, or the continuation
//! backstop (`[goal] max_continuations`); token/time accounting stays visible
//! as telemetry but does not gate continuation — the run is unbounded like
//! grokbuild (`DEFAULT_AGENT_BUDGET` as call cap) and kimicode swarm
//! (`turnBudget` per-task, resumable after budget-reached). Log when the
//! backstop fires.

use std::time::Duration;

/// Default automatic cross-turn continuation policy for one goal run (#5052).
///
/// Goals are unlimited by default: completion, blocked status, or explicit
/// user control ends the run. Operators who want a circuit breaker can opt in
/// with `[goal] max_continuations`; `0` keeps the default unlimited behavior.
pub const DEFAULT_MAX_GOAL_CONTINUATIONS: u32 = 0;

/// Upper bound for one between-turn quiet period. A day is long enough for
/// coordinator cadences while preventing an accidental giant integer from
/// becoming a practically uninterruptible-looking schedule receipt.
pub const MAX_GOAL_CONTINUATION_DELAY_SECONDS: u64 = 24 * 60 * 60;

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
    /// the backstop: only terminal status stops the run.
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

    /// A token budget for telemetry/UI. It never pauses an unbounded goal.
    #[allow(dead_code)]
    pub const fn with_token_budget(token_budget: u64) -> Self {
        Self {
            token_budget: Some(token_budget),
            time_budget_seconds: None,
            max_continuations: DEFAULT_MAX_GOAL_CONTINUATIONS,
        }
    }

    /// Override the continuation backstop (`0` = unlimited until terminal
    /// status).
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
/// 2. The configurable continuation backstop stops a pathological loop
///    (skipped entirely when configured to `0`).
/// 3. Otherwise continue — the loop runs to the completion gate, not to a
///    fixed pass count (#5052). Token/time budgets are advisory telemetry;
///    they are surfaced in the UI but do not stop the run (unbounded).
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

    // 2. Token/time budgets are advisory only (unbounded). They are
    //    visible in the Goal chip + /cost but never pause the loop — like
    //    grokbuild's agent-call budget and kimicode swarm's per-task
    //    turnBudget with resume. Log if we are over budget, then continue.
    if budget
        .token_budget
        .is_some_and(|limit| progress.tokens_used >= limit)
    {
        tracing::debug!(
            tokens_used = progress.tokens_used,
            token_budget = ?budget.token_budget,
            "goal over token budget but continuing (unbounded)"
        );
    }
    if let Some(secs) = budget.time_budget_seconds
        && progress.time_used_seconds >= secs
    {
        tracing::debug!(
            time_used_seconds = progress.time_used_seconds,
            time_budget_seconds = secs,
            "goal over time budget but continuing (unbounded)"
        );
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

/// Outcome of waiting out the between-continuation quiet period.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinuationWaitOutcome {
    /// The quiet period elapsed — dispatch the continuation.
    Elapsed,
    /// Cancelled during the quiet period — never dispatch.
    Cancelled,
}

/// Compute the quiet-period wait for a configured between-continuation delay.
/// `None` continues immediately (unset or zero delay); a positive delay
/// returns the capped wait shared by every dispatch path so no caller can
/// construct an effectively uninterruptible schedule receipt.
#[must_use]
pub const fn continuation_wait(delay_seconds: u64) -> Option<Duration> {
    if delay_seconds == 0 {
        None
    } else {
        Some(Duration::from_secs(
            if delay_seconds > MAX_GOAL_CONTINUATION_DELAY_SECONDS {
                MAX_GOAL_CONTINUATION_DELAY_SECONDS
            } else {
                delay_seconds
            },
        ))
    }
}

/// Wait out the between-continuation quiet period, honoring cancellation.
/// `None` resolves to `Elapsed` immediately so callers have a single dispatch
/// gate. Cancellation is biased and always wins over a racing expiry — the
/// same semantics as the interactive cadence (#5508) for host-managed turns,
/// where the turn loop is the only continuation dispatcher.
pub async fn await_continuation_wait(
    wait: Option<Duration>,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> ContinuationWaitOutcome {
    let Some(wait) = wait else {
        return ContinuationWaitOutcome::Elapsed;
    };
    tokio::select! {
        biased;
        () = cancel_token.cancelled() => ContinuationWaitOutcome::Cancelled,
        () = tokio::time::sleep(wait) => ContinuationWaitOutcome::Elapsed,
    }
}

/// Whether the durable token usage has reached the active goal's budget.
///
/// Budgets are telemetry-only in unbounded goal mode. Keeping this shared
/// predicate false ensures preview and the live continuation loop agree that
/// crossing a token budget does not close the outbound gate.
#[must_use]
pub const fn token_budget_exhausted(_progress: GoalProgress, _budget: GoalBudget) -> bool {
    false
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
    fn default_goal_has_no_continuation_limit() {
        let progress = GoalProgress {
            continuations: 10_000,
            ..GoalProgress::default()
        };
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, GoalBudget::unbounded()),
            ContinuationDecision::Continue
        );
    }

    #[test]
    fn explicit_continuation_limit_stops_run() {
        let configured_limit = 100;
        let progress = GoalProgress {
            continuations: configured_limit,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::unbounded().with_max_continuations(configured_limit);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Stop(StopReason::ContinuationLimit)
        );
    }

    #[test]
    fn operate_goal_continues_past_ten_when_budget_remains() {
        // #5052 regression: the old hardcoded cap of 10 must not be a terminal
        // stop. With no terminal signal, pass 10, 11, and far beyond keep
        // continuing because the default has no hidden ceiling.
        for continuations in [10, 11, 100, 10_000] {
            let progress = GoalProgress {
                tokens_used: 5_000,
                time_used_seconds: 300,
                continuations,
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
    fn zero_backstop_is_unlimited_and_budget_advisory() {
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

        // exceeded token budget is advisory — must still continue (unbounded)
        let progress = GoalProgress {
            tokens_used: 1_000,
            continuations: 10_000,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::with_token_budget(1_000).with_max_continuations(0);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue,
            "budget advisory — must continue even when over budget"
        );
    }

    #[test]
    fn token_budget_is_advisory_not_terminal() {
        let progress = GoalProgress {
            tokens_used: 1000,
            continuations: 1,
            ..GoalProgress::default()
        };
        let budget = GoalBudget::with_token_budget(1000);
        assert_eq!(
            decide_continuation(GoalRunStatus::Active, progress, budget),
            ContinuationDecision::Continue,
            "token budget is advisory — unbounded run must continue"
        );
    }

    #[test]
    fn time_budget_is_advisory_not_terminal() {
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
            ContinuationDecision::Continue,
            "time budget is advisory — unbounded run must continue"
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

    #[test]
    fn continuation_wait_honors_configured_delay() {
        assert_eq!(
            continuation_wait(300),
            Some(Duration::from_secs(300)),
            "a positive configured delay must become the quiet-period wait"
        );
        assert_eq!(
            continuation_wait(MAX_GOAL_CONTINUATION_DELAY_SECONDS + 1),
            Some(Duration::from_secs(MAX_GOAL_CONTINUATION_DELAY_SECONDS)),
            "the shared cap must bound an oversized configured delay"
        );
    }

    #[test]
    fn zero_delay_continues_immediately() {
        assert_eq!(
            continuation_wait(0),
            None,
            "an unset or zero delay must dispatch immediately"
        );
    }

    #[tokio::test]
    async fn cancellation_wins_over_pending_quiet_period() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        let canceller = cancel_token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            canceller.cancel();
        });
        assert_eq!(
            await_continuation_wait(
                continuation_wait(MAX_GOAL_CONTINUATION_DELAY_SECONDS),
                &cancel_token,
            )
            .await,
            ContinuationWaitOutcome::Cancelled,
            "an explicit cancel during the quiet period must win and never dispatch"
        );
    }

    #[tokio::test]
    async fn elapsed_quiet_period_dispatches() {
        let cancel_token = tokio_util::sync::CancellationToken::new();
        assert_eq!(
            await_continuation_wait(None, &cancel_token).await,
            ContinuationWaitOutcome::Elapsed,
            "an unset wait must gate the dispatch through immediately"
        );
        assert_eq!(
            await_continuation_wait(Some(Duration::from_millis(1)), &cancel_token).await,
            ContinuationWaitOutcome::Elapsed,
            "an expired quiet period must dispatch"
        );
    }
}
