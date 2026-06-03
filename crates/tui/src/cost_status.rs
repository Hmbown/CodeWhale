//! Process-wide cost-accrual side-channel (#526).
//!
//! Background LLM calls outside the main turn-complete path
//! (compaction summaries, seam recompaction) used
//! to drop their token usage on the floor — the dashboard's
//! session-cost only saw the parent turn's tokens, so a long
//! session that triggered compaction under-reported
//! cost by however many tokens those background calls consumed.
//!
//! Mirrors the [`crate::retry_status`] pattern: background callers
//! call [`report`] after each `client.create_message`, the TUI
//! render loop calls [`drain`] every frame, and any drained amount
//! gets folded into `App::accrue_subagent_cost_estimate`.
//!
//! Why a side-channel and not a plumbed callback: the leaky callers
//! (`compaction.rs`, `seam_manager.rs`) are
//! engine-internal machinery without a direct handle to `App` or
//! the engine's event channel. A side-channel keeps the change
//! surface tiny — one new `report` line per call site — and any
//! future background caller (summarizers, retrieval helpers) gets
//! accrued for free without further plumbing.

use std::sync::{Mutex, OnceLock};

use crate::models::Usage;
use crate::pricing::CostEstimate;

static PENDING: OnceLock<Mutex<CostEstimate>> = OnceLock::new();

fn cell() -> &'static Mutex<CostEstimate> {
    PENDING.get_or_init(|| Mutex::new(CostEstimate::default()))
}

/// Background callers report their LLM usage here. Computes the
/// cost via [`crate::pricing::calculate_turn_cost_estimate_from_usage`] and
/// adds it to the pending pool. Cheap; takes a short-lived lock
/// and returns. No-op on models the pricing table doesn't know.
pub fn report(model: &str, usage: &Usage) {
    let Some(cost) = crate::pricing::calculate_turn_cost_estimate_from_usage(model, usage) else {
        return;
    };
    if !cost.is_positive() {
        return;
    }
    if let Ok(mut pending) = cell().lock() {
        pending.usd += cost.usd;
        pending.cny += cost.cny;
    }
}

/// Drain the pending cost. Returns the accumulated amount and resets
/// the pool to zero. Called by the TUI render / event loop on each
/// frame; any non-zero result gets folded into `accrue_subagent_cost_estimate`.
pub fn drain() -> CostEstimate {
    let Ok(mut pending) = cell().lock() else {
        return CostEstimate::default();
    };
    std::mem::take(&mut *pending)
}

/// Reset the pool to zero without consuming. Test-only helper for
/// suites that share the static and need to start from a known
/// state. Production code should always use [`drain`].
#[cfg(test)]
pub fn reset_for_tests() {
    if let Ok(mut pending) = cell().lock() {
        *pending = CostEstimate::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_usage() -> Usage {
        Usage {
            input_tokens: 1_000,
            output_tokens: 500,
            ..Default::default()
        }
    }

    /// Tests run in parallel and share the static — serialize the
    /// ones that touch the pool through this mutex so concurrent
    /// `report`/`drain` doesn't make assertions racy.
    fn serial_lock() -> std::sync::MutexGuard<'static, ()> {
        static M: OnceLock<Mutex<()>> = OnceLock::new();
        M.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn report_adds_to_pool_and_drain_returns_then_resets() {
        let _g = serial_lock();
        reset_for_tests();
        report("deepseek-v4-flash", &small_usage());
        let first = drain();
        assert!(first.usd > 0.0, "expected positive USD cost, got {first:?}");
        assert!(first.cny > 0.0, "expected positive CNY cost, got {first:?}");
        let second = drain();
        assert_eq!(second, CostEstimate::default(), "drain must zero the pool");
    }

    #[test]
    fn report_skips_unknown_models() {
        let _g = serial_lock();
        reset_for_tests();
        // NIM-hosted models intentionally have no DeepSeek pricing.
        report("deepseek-ai/deepseek-v4-pro", &small_usage());
        assert_eq!(drain(), CostEstimate::default());
    }

    #[test]
    fn report_accumulates_across_multiple_calls() {
        let _g = serial_lock();
        reset_for_tests();
        report("deepseek-v4-flash", &small_usage());
        report("deepseek-v4-flash", &small_usage());
        let total = drain();
        // Two equal reports — total must be 2× a single report.
        let single = crate::pricing::calculate_turn_cost_estimate_from_usage(
            "deepseek-v4-flash",
            &small_usage(),
        )
        .unwrap();
        assert!((total.usd - 2.0 * single.usd).abs() < 1e-12);
        assert!((total.cny - 2.0 * single.cny).abs() < 1e-12);
    }
}
