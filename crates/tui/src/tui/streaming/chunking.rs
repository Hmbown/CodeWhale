//! Adaptive stream chunking policy for two-gear streaming.
//!
//! Ported from `codex-rs/tui/src/streaming/chunking.rs`, adapted for deepseek-tui's
//! text-based streaming pipeline. The policy is queue-pressure driven and
//! source-agnostic.
//!
//! # Mental model
//!
//! Two gears:
//! - [`ChunkingMode::Smooth`]: normal pressure.
//! - [`ChunkingMode::CatchUp`]: elevated pressure.
//!
//! Normal-motion callers drain all currently available chunks so the display
//! follows the upstream SSE delta cadence. Low-motion callers stay in Smooth
//! and drain one chunk per tick to reduce visual churn.
//!
//! # Hysteresis
//!
//! - Enter `CatchUp` when `queued_lines >= ENTER_QUEUE_DEPTH_LINES` OR
//!   the oldest queued chunk is at least [`ENTER_OLDEST_AGE`].
//! - Exit `CatchUp` only after pressure stays below [`EXIT_QUEUE_DEPTH_LINES`]
//!   AND [`EXIT_OLDEST_AGE`] for at least [`EXIT_HOLD`].
//! - After exit, suppress immediate re-entry for [`REENTER_CATCH_UP_HOLD`]
//!   unless backlog is "severe" (queue >= [`SEVERE_QUEUE_DEPTH_LINES`] or
//!   oldest >= [`SEVERE_OLDEST_AGE`]).

use std::time::Duration;
use std::time::Instant;

/// Queue-depth threshold that allows entering catch-up mode.
pub(crate) const ENTER_QUEUE_DEPTH_LINES: usize = 160;

/// Oldest-chunk age threshold that allows entering catch-up mode.
pub(crate) const ENTER_OLDEST_AGE: Duration = Duration::from_millis(1_200);

/// Queue-depth threshold used when evaluating catch-up exit hysteresis.
pub(crate) const EXIT_QUEUE_DEPTH_LINES: usize = 32;

/// Oldest-chunk age threshold used when evaluating catch-up exit hysteresis.
pub(crate) const EXIT_OLDEST_AGE: Duration = Duration::from_millis(300);

/// Minimum duration queue pressure must stay below exit thresholds to leave catch-up mode.
pub(crate) const EXIT_HOLD: Duration = Duration::from_millis(250);

/// Cooldown window after a catch-up exit that suppresses immediate re-entry.
pub(crate) const REENTER_CATCH_UP_HOLD: Duration = Duration::from_millis(250);

/// Queue-depth cutoff that marks backlog as severe (bypasses re-entry hold).
pub(crate) const SEVERE_QUEUE_DEPTH_LINES: usize = 640;

/// Oldest-line age cutoff that marks backlog as severe.
pub(crate) const SEVERE_OLDEST_AGE: Duration = Duration::from_millis(4_000);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChunkingMode {
    /// Drain one display chunk per baseline commit tick.
    #[default]
    Smooth,
    /// Drain the queued backlog according to queue pressure.
    CatchUp,
}

/// Captures queue pressure inputs used by adaptive chunking decisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueSnapshot {
    /// Number of queued stream chunks waiting to be displayed.
    pub queued_lines: usize,
    /// Age of the oldest queued chunk at decision time.
    pub oldest_age: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrainPlan {
    /// Emit all queued chunks available at this tick.
    Available,
    /// Emit exactly one queued line.
    Single,
}

/// Represents one policy decision for a specific queue snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkingDecision {
    /// Mode after applying hysteresis transitions for this decision.
    pub mode: ChunkingMode,
    /// Whether this decision transitioned from `Smooth` into `CatchUp`.
    pub entered_catch_up: bool,
    /// Drain plan to execute for the current commit tick.
    pub drain_plan: DrainPlan,
}

/// Maintains adaptive chunking mode and hysteresis state across ticks.
#[derive(Debug, Default, Clone)]
pub struct AdaptiveChunkingPolicy {
    mode: ChunkingMode,
    below_exit_threshold_since: Option<Instant>,
    last_catch_up_exit_at: Option<Instant>,
    /// When true, the policy never enters `CatchUp` — it stays in `Smooth`
    /// regardless of queue pressure, keeping the display calm for users who
    /// prefer reduced visual churn.
    low_motion: bool,
}

impl AdaptiveChunkingPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the policy mode used by the most recent decision.
    pub fn mode(&self) -> ChunkingMode {
        self.mode
    }

    /// Resets state to baseline smooth mode.
    pub fn reset(&mut self) {
        self.mode = ChunkingMode::Smooth;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
    }

    /// When true, the policy never enters `CatchUp` — it stays in `Smooth`
    /// regardless of queue pressure.
    pub fn set_low_motion(&mut self, low_motion: bool) {
        self.low_motion = low_motion;
        if low_motion {
            self.mode = ChunkingMode::Smooth;
            self.below_exit_threshold_since = None;
            self.last_catch_up_exit_at = None;
        }
    }

    /// Computes a drain decision from the current queue snapshot.
    pub fn decide(&mut self, snapshot: QueueSnapshot, now: Instant) -> ChunkingDecision {
        // In low-motion mode, always use Smooth pacing regardless of queue
        // pressure — the user asked for a calm, steady display.
        if self.low_motion {
            self.mode = ChunkingMode::Smooth;
            self.below_exit_threshold_since = None;
            return ChunkingDecision {
                mode: self.mode,
                entered_catch_up: false,
                drain_plan: DrainPlan::Single,
            };
        }

        if snapshot.queued_lines == 0 {
            self.note_catch_up_exit(now);
            self.mode = ChunkingMode::Smooth;
            self.below_exit_threshold_since = None;
            return ChunkingDecision {
                mode: self.mode,
                entered_catch_up: false,
                drain_plan: DrainPlan::Available,
            };
        }

        let entered_catch_up = match self.mode {
            ChunkingMode::Smooth => self.maybe_enter_catch_up(snapshot, now),
            ChunkingMode::CatchUp => {
                self.maybe_exit_catch_up(snapshot, now);
                false
            }
        };

        ChunkingDecision {
            mode: self.mode,
            entered_catch_up,
            drain_plan: DrainPlan::Available,
        }
    }

    fn maybe_enter_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) -> bool {
        if !should_enter_catch_up(snapshot) {
            return false;
        }
        if self.reentry_hold_active(now) && !is_severe_backlog(snapshot) {
            return false;
        }
        self.mode = ChunkingMode::CatchUp;
        self.below_exit_threshold_since = None;
        self.last_catch_up_exit_at = None;
        true
    }

    fn maybe_exit_catch_up(&mut self, snapshot: QueueSnapshot, now: Instant) {
        if !should_exit_catch_up(snapshot) {
            self.below_exit_threshold_since = None;
            return;
        }

        match self.below_exit_threshold_since {
            Some(since) if now.saturating_duration_since(since) >= EXIT_HOLD => {
                self.mode = ChunkingMode::Smooth;
                self.below_exit_threshold_since = None;
                self.last_catch_up_exit_at = Some(now);
            }
            Some(_) => {}
            None => {
                self.below_exit_threshold_since = Some(now);
            }
        }
    }

    fn note_catch_up_exit(&mut self, now: Instant) {
        if self.mode == ChunkingMode::CatchUp {
            self.last_catch_up_exit_at = Some(now);
        }
    }

    fn reentry_hold_active(&self, now: Instant) -> bool {
        self.last_catch_up_exit_at
            .is_some_and(|exit| now.saturating_duration_since(exit) < REENTER_CATCH_UP_HOLD)
    }
}

/// Returns whether current queue pressure warrants entering catch-up mode.
fn should_enter_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= ENTER_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= ENTER_OLDEST_AGE)
}

/// Returns whether queue pressure is low enough to begin exit hysteresis.
fn should_exit_catch_up(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines <= EXIT_QUEUE_DEPTH_LINES
        && snapshot
            .oldest_age
            .is_some_and(|oldest| oldest <= EXIT_OLDEST_AGE)
}

/// Returns whether backlog is severe enough to bypass the re-entry hold.
fn is_severe_backlog(snapshot: QueueSnapshot) -> bool {
    snapshot.queued_lines >= SEVERE_QUEUE_DEPTH_LINES
        || snapshot
            .oldest_age
            .is_some_and(|oldest| oldest >= SEVERE_OLDEST_AGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(queued_lines: usize, oldest_age_ms: u64) -> QueueSnapshot {
        QueueSnapshot {
            queued_lines,
            oldest_age: Some(Duration::from_millis(oldest_age_ms)),
        }
    }

    fn empty_snap() -> QueueSnapshot {
        QueueSnapshot {
            queued_lines: 0,
            oldest_age: None,
        }
    }

    #[test]
    fn smooth_only_burst_drains_available_chunks_in_normal_motion() {
        // Five slowly-arriving lines, each well below enter thresholds, never
        // flip the policy out of `Smooth`. Normal motion still drains what is
        // already available so display pacing follows upstream deltas.
        let mut policy = AdaptiveChunkingPolicy::new();
        let t0 = Instant::now();

        for i in 0..5 {
            // 1 queued line, age 10 ms — far below ENTER thresholds.
            let decision = policy.decide(snap(1, 10), t0 + Duration::from_millis(50 * i));
            assert_eq!(decision.mode, ChunkingMode::Smooth);
            assert!(!decision.entered_catch_up);
            assert_eq!(decision.drain_plan, DrainPlan::Available);
        }
    }

    #[test]
    fn deep_burst_flips_to_catch_up_and_drains_backlog() {
        // A burst crossing ENTER_QUEUE_DEPTH_LINES enters CatchUp. With
        // single-grapheme chunks, the threshold stays high enough that
        // ordinary prose still drips in visibly before catch-up engages.
        // The policy should enter `CatchUp`, while normal-motion draining still
        // preserves the already-arrived upstream burst.
        let mut policy = AdaptiveChunkingPolicy::new();
        let now = Instant::now();

        let decision = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES, 10), now);
        assert_eq!(decision.mode, ChunkingMode::CatchUp);
        assert!(decision.entered_catch_up);
        assert_eq!(decision.drain_plan, DrainPlan::Available);

        // Larger backlog requested next tick: still CatchUp, batch grows to match.
        let larger_backlog = ENTER_QUEUE_DEPTH_LINES + 80;
        let decision = policy.decide(snap(larger_backlog, 30), now + Duration::from_millis(10));
        assert_eq!(decision.mode, ChunkingMode::CatchUp);
        assert!(!decision.entered_catch_up, "no second transition signal");
        assert_eq!(decision.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn age_threshold_alone_triggers_catch_up() {
        // Queue depth is small, but the oldest chunk has crossed the age threshold.
        // Either condition is sufficient to enter catch-up.
        let mut policy = AdaptiveChunkingPolicy::new();
        let now = Instant::now();

        let decision = policy.decide(snap(2, ENTER_OLDEST_AGE.as_millis() as u64), now);
        assert_eq!(decision.mode, ChunkingMode::CatchUp);
        assert!(decision.entered_catch_up);
        assert_eq!(decision.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn catch_up_exits_after_low_activity_hold() {
        // Enter CatchUp via depth burst, then drop pressure below exit
        // thresholds. Policy must hold for >=EXIT_HOLD before returning to Smooth.
        let mut policy = AdaptiveChunkingPolicy::new();
        let t0 = Instant::now();

        let _ = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES, 20), t0);
        assert_eq!(policy.mode(), ChunkingMode::CatchUp);

        // Pressure drops to the exit thresholds.
        // Hold begins; not yet 250ms.
        let pre_hold = policy.decide(
            snap(EXIT_QUEUE_DEPTH_LINES, EXIT_OLDEST_AGE.as_millis() as u64),
            t0 + Duration::from_millis(50),
        );
        assert_eq!(pre_hold.mode, ChunkingMode::CatchUp);

        // Still under hold.
        let mid_hold = policy.decide(
            snap(EXIT_QUEUE_DEPTH_LINES, EXIT_OLDEST_AGE.as_millis() as u64),
            t0 + Duration::from_millis(200),
        );
        assert_eq!(mid_hold.mode, ChunkingMode::CatchUp);

        // Past EXIT_HOLD (250 ms) → return to Smooth.
        let post_hold = policy.decide(
            snap(EXIT_QUEUE_DEPTH_LINES, EXIT_OLDEST_AGE.as_millis() as u64),
            t0 + Duration::from_millis(320),
        );
        assert_eq!(post_hold.mode, ChunkingMode::Smooth);
        assert_eq!(post_hold.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn idle_resets_to_smooth_immediately() {
        // An empty queue forces Smooth regardless of prior mode.
        let mut policy = AdaptiveChunkingPolicy::new();
        let now = Instant::now();

        let _ = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES, 20), now);
        assert_eq!(policy.mode(), ChunkingMode::CatchUp);

        let decision = policy.decide(empty_snap(), now + Duration::from_millis(10));
        assert_eq!(decision.mode, ChunkingMode::Smooth);
        assert_eq!(decision.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn reentry_hold_blocks_immediate_flip_back() {
        // After exiting CatchUp via idle, a threshold-sized burst that arrives within
        // the re-entry hold window should not immediately re-enter CatchUp.
        let mut policy = AdaptiveChunkingPolicy::new();
        let t0 = Instant::now();

        let _ = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES, 20), t0);
        let _ = policy.decide(empty_snap(), t0 + Duration::from_millis(10));

        // Within REENTER_CATCH_UP_HOLD (250 ms): hold blocks re-entry.
        let held = policy.decide(
            snap(ENTER_QUEUE_DEPTH_LINES, 20),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(held.mode, ChunkingMode::Smooth);
        assert_eq!(held.drain_plan, DrainPlan::Available);

        // Past the hold: re-entry permitted.
        let reentered = policy.decide(
            snap(ENTER_QUEUE_DEPTH_LINES, 20),
            t0 + Duration::from_millis(400),
        );
        assert_eq!(reentered.mode, ChunkingMode::CatchUp);
        assert_eq!(reentered.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn severe_backlog_bypasses_reentry_hold() {
        // Even within the hold window, a "severe" backlog bypasses
        // the gate so display lag doesn't unbounded-grow.
        let mut policy = AdaptiveChunkingPolicy::new();
        let t0 = Instant::now();

        let _ = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES, 20), t0);
        let _ = policy.decide(empty_snap(), t0 + Duration::from_millis(10));

        let severe = policy.decide(
            snap(SEVERE_QUEUE_DEPTH_LINES, 20),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(severe.mode, ChunkingMode::CatchUp);
        assert_eq!(severe.drain_plan, DrainPlan::Available);
    }

    #[test]
    fn low_motion_always_smooth_regardless_of_pressure() {
        let mut policy = AdaptiveChunkingPolicy::new();
        policy.set_low_motion(true);
        let t0 = Instant::now();

        // Queue depth far above ENTER threshold.
        let d1 = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES + 80, 10), t0);
        assert_eq!(d1.mode, ChunkingMode::Smooth);
        assert!(!d1.entered_catch_up);
        assert_eq!(d1.drain_plan, DrainPlan::Single);

        // Oldest age far above ENTER threshold.
        let d2 = policy.decide(
            snap(5, ENTER_OLDEST_AGE.as_millis() as u64),
            t0 + Duration::from_millis(100),
        );
        assert_eq!(d2.mode, ChunkingMode::Smooth);
        assert!(!d2.entered_catch_up);
        assert_eq!(d2.drain_plan, DrainPlan::Single);

        // Severe backlog — still Smooth.
        let d3 = policy.decide(
            snap(
                SEVERE_QUEUE_DEPTH_LINES + 80,
                SEVERE_OLDEST_AGE.as_millis() as u64,
            ),
            t0 + Duration::from_millis(200),
        );
        assert_eq!(d3.mode, ChunkingMode::Smooth);
        assert_eq!(d3.drain_plan, DrainPlan::Single);
    }

    #[test]
    fn low_motion_reset_resumes_normal_operation() {
        let mut policy = AdaptiveChunkingPolicy::new();
        policy.set_low_motion(true);
        let t0 = Instant::now();

        // Low motion blocks catch-up.
        let d1 = policy.decide(snap(ENTER_QUEUE_DEPTH_LINES + 80, 10), t0);
        assert_eq!(d1.mode, ChunkingMode::Smooth);

        // Turn off low motion — next burst should enter CatchUp.
        policy.set_low_motion(false);
        let d2 = policy.decide(
            snap(ENTER_QUEUE_DEPTH_LINES + 80, 10),
            t0 + Duration::from_millis(10),
        );
        assert_eq!(d2.mode, ChunkingMode::CatchUp);
        assert!(d2.entered_catch_up);
        assert_eq!(d2.drain_plan, DrainPlan::Available);
    }
}
