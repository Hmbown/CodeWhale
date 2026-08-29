//! Finite turn budgets (R1, v0.9.12 Phase 1).
//!
//! An agent loop with no finite bound can spend real money forever. Before
//! R1 the parent turn had exactly one hard ceiling — the per-step stream
//! caps in [`super::streaming`] — while the model-step ceiling defaulted to
//! `u32::MAX` at every production call site and no cumulative per-turn
//! wall-clock budget existed at all. This module is the single home for the
//! four bounds R1 makes finite:
//!
//! 1. `max_steps` — model steps in one turn.
//! 2. The cumulative per-turn wall clock.
//! 3. `exec --max-turns` — the same step ceiling for headless runs.
//! 4. The per-step stream caps (content bytes, stream duration).
//!
//! ## No `0`-means-unlimited sentinel
//!
//! Every resolver here rejects `0` and falls back to the finite default
//! rather than reading it as "unlimited". That sentinel is exactly the bug
//! class R1 exists to close: a `0` that skips the cap check turns a
//! misconfiguration (or a typo) into an unbounded spend. There is also no
//! "unlimited" value at all — a caller who genuinely wants a turn to run
//! practically forever raises the knob to its documented maximum, which is
//! large but still finite and still terminates.
//!
//! ## Honesty at the limit
//!
//! Hitting a budget is never a clean success. The step ceiling already ends
//! the turn as `TurnOutcomeStatus::Failed` with the limit named (or, when
//! the model still owes work, grants exactly one bounded final-report turn
//! first). [`TurnWallClock`] follows the same contract in `run_turn`.

use std::time::{Duration, Instant};

/// Default ceiling on model steps within a single turn.
///
/// A "step" is one accepted provider response, so this bounds how many
/// billable requests one user message can trigger. 200 is far above what an
/// ordinary interactive turn spends and still finite: a runaway tool loop
/// stops here instead of spending until the operator notices.
pub const DEFAULT_MAX_MODEL_STEPS: u32 = 200;
/// Smallest accepted model-step ceiling. One step still lets the model
/// answer once.
pub const MIN_MAX_MODEL_STEPS: u32 = 1;
/// Largest accepted model-step ceiling. Deliberately large enough to serve
/// as the "effectively unlimited" escape hatch while remaining finite, so
/// no configuration path can produce an unbounded loop.
pub const MAX_MAX_MODEL_STEPS: u32 = 100_000;

/// Default headless `exec --max-turns`. Same ceiling as the interactive
/// engine: a non-interactive run has nobody watching it, so it must not be
/// looser than the one a human is sitting in front of.
pub const DEFAULT_EXEC_MAX_TURNS: u32 = DEFAULT_MAX_MODEL_STEPS;

/// Default cumulative per-turn wall-clock budget, in seconds.
///
/// Measured across every model step of one turn, not per request. Time the
/// turn spends blocked on a human approval decision is excluded (see
/// [`TurnWallClock::begin_human_wait`]) so an unanswered prompt cannot
/// consume the budget.
pub const DEFAULT_TURN_WALL_CLOCK_SECS: u64 = 3_600;
/// Smallest accepted per-turn wall-clock budget. Below this a single slow
/// reasoning request would trip the budget before it could finish.
pub const MIN_TURN_WALL_CLOCK_SECS: u64 = 30;
/// Largest accepted per-turn wall-clock budget (24 hours).
pub const MAX_TURN_WALL_CLOCK_SECS: u64 = 86_400;

/// Default per-step cap on accumulated streamed content, in bytes.
/// Preserves the pre-R1 hard-coded value; R1 only makes it overridable.
pub const DEFAULT_STREAM_MAX_CONTENT_BYTES: usize = super::streaming::STREAM_MAX_CONTENT_BYTES;
/// Smallest accepted per-step stream content cap (64 KiB).
pub const MIN_STREAM_MAX_CONTENT_BYTES: usize = 64 * 1024;
/// Largest accepted per-step stream content cap (512 MiB).
pub const MAX_STREAM_MAX_CONTENT_BYTES: usize = 512 * 1024 * 1024;

/// Default per-step cap on a single stream's wall-clock duration, in
/// seconds. Preserves the pre-R1 hard-coded value.
pub const DEFAULT_STREAM_MAX_DURATION_SECS: u64 = super::streaming::STREAM_MAX_DURATION_SECS;
/// Smallest accepted per-step stream duration cap.
pub const MIN_STREAM_MAX_DURATION_SECS: u64 = 10;
/// Largest accepted per-step stream duration cap (24 hours).
pub const MAX_STREAM_MAX_DURATION_SECS: u64 = 86_400;

/// Resolve a configured model-step ceiling.
///
/// `None` and `0` both resolve to [`DEFAULT_MAX_MODEL_STEPS`]: `0` is
/// treated as an invalid value, never as "unlimited". Positive values clamp
/// into `MIN_MAX_MODEL_STEPS..=MAX_MAX_MODEL_STEPS`, so even the largest
/// configurable turn terminates.
#[must_use]
pub fn resolve_max_model_steps(raw: Option<u32>) -> u32 {
    match raw {
        None | Some(0) => DEFAULT_MAX_MODEL_STEPS,
        Some(value) => value.clamp(MIN_MAX_MODEL_STEPS, MAX_MAX_MODEL_STEPS),
    }
}

/// Resolve a configured per-turn wall-clock budget, in seconds.
///
/// `None` and `0` both resolve to [`DEFAULT_TURN_WALL_CLOCK_SECS`]; `0` is
/// invalid, not "unlimited".
#[must_use]
pub fn resolve_turn_wall_clock_secs(raw: Option<u64>) -> u64 {
    match raw {
        None | Some(0) => DEFAULT_TURN_WALL_CLOCK_SECS,
        Some(value) => value.clamp(MIN_TURN_WALL_CLOCK_SECS, MAX_TURN_WALL_CLOCK_SECS),
    }
}

/// Resolve a configured per-turn wall-clock budget as a [`Duration`].
#[must_use]
pub fn resolve_turn_wall_clock(raw: Option<u64>) -> Duration {
    Duration::from_secs(resolve_turn_wall_clock_secs(raw))
}

/// Resolve a configured per-step stream content cap, given megabytes.
///
/// `None` and `0` both resolve to [`DEFAULT_STREAM_MAX_CONTENT_BYTES`].
#[must_use]
pub fn resolve_stream_max_content_bytes(raw_mb: Option<u64>) -> usize {
    match raw_mb {
        None | Some(0) => DEFAULT_STREAM_MAX_CONTENT_BYTES,
        Some(mb) => usize::try_from(mb.saturating_mul(1024 * 1024))
            .unwrap_or(MAX_STREAM_MAX_CONTENT_BYTES)
            .clamp(MIN_STREAM_MAX_CONTENT_BYTES, MAX_STREAM_MAX_CONTENT_BYTES),
    }
}

/// Resolve a configured per-step stream duration cap, in seconds.
///
/// `None` and `0` both resolve to [`DEFAULT_STREAM_MAX_DURATION_SECS`].
#[must_use]
pub fn resolve_stream_max_duration_secs(raw: Option<u64>) -> u64 {
    match raw {
        None | Some(0) => DEFAULT_STREAM_MAX_DURATION_SECS,
        Some(value) => value.clamp(MIN_STREAM_MAX_DURATION_SECS, MAX_STREAM_MAX_DURATION_SECS),
    }
}

/// Cumulative wall-clock budget for one turn.
///
/// Started once at the top of `Engine::run_turn` and checked at the
/// provider-request boundary, so a turn that trips the budget stops before
/// authorizing another billable request and keeps every tool result already
/// in the transcript.
///
/// Time blocked on a human approval decision is excluded: the budget bounds
/// what the agent spends on its own, not how long a person takes to answer.
/// Without that exclusion an approval prompt left open overnight would fail
/// the turn — and discard the work the user just approved — the moment they
/// came back.
#[derive(Debug)]
pub(crate) struct TurnWallClock {
    budget: Duration,
    started_at: Instant,
    /// Total time already excluded because the turn was blocked on a human.
    excluded: Duration,
    /// Set while currently blocked on a human decision.
    blocked_since: Option<Instant>,
}

impl TurnWallClock {
    /// Start a fresh budget. A zero budget is legal here (and only here):
    /// it is how tests assert the stop path without sleeping. Configuration
    /// never produces one — [`resolve_turn_wall_clock`] rejects `0`.
    pub(crate) fn start(budget: Duration) -> Self {
        Self {
            budget,
            started_at: Instant::now(),
            excluded: Duration::ZERO,
            blocked_since: None,
        }
    }

    /// The budget this clock was started with.
    pub(crate) fn budget(&self) -> Duration {
        self.budget
    }

    /// Wall-clock time this turn has spent on its own work, excluding time
    /// blocked on a human decision.
    pub(crate) fn spent(&self) -> Duration {
        let blocked_now = self
            .blocked_since
            .map_or(Duration::ZERO, |since| since.elapsed());
        self.started_at
            .elapsed()
            .saturating_sub(self.excluded)
            .saturating_sub(blocked_now)
    }

    /// Whether the cumulative budget is spent.
    pub(crate) fn exhausted(&self) -> bool {
        self.spent() >= self.budget
    }

    /// Stop counting: the turn is now waiting on a human decision.
    /// Idempotent — a second call while already blocked does nothing, so a
    /// nested or re-entered approval cannot double-exclude.
    pub(crate) fn begin_human_wait(&mut self) {
        if self.blocked_since.is_none() {
            self.blocked_since = Some(Instant::now());
        }
    }

    /// Resume counting after a human decision, banking the blocked time.
    pub(crate) fn end_human_wait(&mut self) {
        if let Some(since) = self.blocked_since.take() {
            self.excluded = self.excluded.saturating_add(since.elapsed());
        }
    }

    /// Test-only: pretend `elapsed` more wall-clock time has passed, so the
    /// exhaustion path can be exercised without sleeping. An in-progress
    /// human wait is rewound too — otherwise the simulated time would count
    /// as agent-owned work that never actually happened.
    #[cfg(test)]
    pub(crate) fn rewind_for_test(&mut self, elapsed: Duration) {
        self.started_at = self
            .started_at
            .checked_sub(elapsed)
            .unwrap_or(self.started_at);
        if let Some(since) = self.blocked_since {
            self.blocked_since = Some(since.checked_sub(elapsed).unwrap_or(since));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_step_defaults_are_finite_and_reject_the_zero_sentinel() {
        assert_eq!(resolve_max_model_steps(None), DEFAULT_MAX_MODEL_STEPS);
        // `0` is invalid, not "unlimited" — the trap R1 exists to close.
        assert_eq!(resolve_max_model_steps(Some(0)), DEFAULT_MAX_MODEL_STEPS);
        const { assert!(DEFAULT_MAX_MODEL_STEPS < u32::MAX) };
        const { assert!(MAX_MAX_MODEL_STEPS < u32::MAX) };
    }

    #[test]
    fn model_step_ceiling_is_overridable_and_clamped() {
        assert_eq!(resolve_max_model_steps(Some(7)), 7);
        assert_eq!(resolve_max_model_steps(Some(1)), 1);
        assert_eq!(
            resolve_max_model_steps(Some(u32::MAX)),
            MAX_MAX_MODEL_STEPS,
            "the escape hatch is large but still finite"
        );
    }

    #[test]
    fn turn_wall_clock_defaults_are_finite_and_reject_the_zero_sentinel() {
        assert_eq!(
            resolve_turn_wall_clock_secs(None),
            DEFAULT_TURN_WALL_CLOCK_SECS
        );
        assert_eq!(
            resolve_turn_wall_clock_secs(Some(0)),
            DEFAULT_TURN_WALL_CLOCK_SECS
        );
        assert_eq!(
            resolve_turn_wall_clock(None),
            Duration::from_secs(DEFAULT_TURN_WALL_CLOCK_SECS)
        );
    }

    #[test]
    fn turn_wall_clock_is_overridable_and_clamped() {
        assert_eq!(resolve_turn_wall_clock_secs(Some(120)), 120);
        assert_eq!(
            resolve_turn_wall_clock_secs(Some(1)),
            MIN_TURN_WALL_CLOCK_SECS
        );
        assert_eq!(
            resolve_turn_wall_clock_secs(Some(u64::MAX)),
            MAX_TURN_WALL_CLOCK_SECS
        );
    }

    #[test]
    fn stream_caps_default_reject_zero_and_are_overridable() {
        assert_eq!(
            resolve_stream_max_content_bytes(None),
            DEFAULT_STREAM_MAX_CONTENT_BYTES
        );
        assert_eq!(
            resolve_stream_max_content_bytes(Some(0)),
            DEFAULT_STREAM_MAX_CONTENT_BYTES
        );
        assert_eq!(resolve_stream_max_content_bytes(Some(1)), 1024 * 1024);
        assert_eq!(
            resolve_stream_max_content_bytes(Some(u64::MAX)),
            MAX_STREAM_MAX_CONTENT_BYTES
        );

        assert_eq!(
            resolve_stream_max_duration_secs(None),
            DEFAULT_STREAM_MAX_DURATION_SECS
        );
        assert_eq!(
            resolve_stream_max_duration_secs(Some(0)),
            DEFAULT_STREAM_MAX_DURATION_SECS
        );
        assert_eq!(resolve_stream_max_duration_secs(Some(60)), 60);
        assert_eq!(
            resolve_stream_max_duration_secs(Some(u64::MAX)),
            MAX_STREAM_MAX_DURATION_SECS
        );
    }

    #[test]
    fn wall_clock_exhausts_once_the_budget_is_spent() {
        let mut clock = TurnWallClock::start(Duration::from_secs(60));
        assert!(!clock.exhausted());
        clock.rewind_for_test(Duration::from_secs(61));
        assert!(clock.exhausted());
        assert!(clock.spent() >= clock.budget());
    }

    #[test]
    fn wall_clock_excludes_time_blocked_on_a_human_decision() {
        let mut clock = TurnWallClock::start(Duration::from_secs(60));
        clock.begin_human_wait();
        // The human took two minutes; the agent spent none of its budget.
        clock.rewind_for_test(Duration::from_secs(120));
        assert!(
            !clock.exhausted(),
            "an unanswered approval prompt must not burn the turn budget"
        );
        clock.end_human_wait();
        assert!(!clock.exhausted());
        // Agent-owned time after the decision still counts.
        clock.rewind_for_test(Duration::from_secs(61));
        assert!(clock.exhausted());
    }

    #[test]
    fn nested_human_waits_cannot_double_exclude() {
        let mut clock = TurnWallClock::start(Duration::from_secs(60));
        clock.begin_human_wait();
        clock.begin_human_wait();
        clock.rewind_for_test(Duration::from_secs(30));
        clock.end_human_wait();
        // A second unmatched end is a no-op, not another exclusion.
        clock.end_human_wait();
        clock.rewind_for_test(Duration::from_secs(61));
        assert!(clock.exhausted());
    }
}
