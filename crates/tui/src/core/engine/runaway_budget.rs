//! Finite default step/wall-clock bounds for ordinary hosts (R1 / #5566).
//!
//! The per-step stream duration cap still exists, but it is clamped to the
//! remaining cumulative per-turn wall-clock so a multi-step turn cannot
//! re-arm 30 minutes of spend on every model call.

use std::time::Duration;

/// Finite default model-step ceiling for ordinary TUI and exec hosts.
pub(crate) const DEFAULT_MAX_STEPS: u32 = 100;

/// Cumulative per-turn wall-clock bound. `Duration::ZERO` on
/// `EngineConfig::max_wall_time` disables the bound.
pub(crate) const DEFAULT_MAX_WALL_TIME: Duration = Duration::from_secs(30 * 60);

/// Remaining wall-clock allowed for the current model stream.
///
/// The per-step cap still applies, but it cannot re-arm past the turn's
/// cumulative wall-clock. `max_wall_time == Duration::ZERO` means the turn
/// has no cumulative bound (per-step cap only).
pub(crate) fn stream_duration_limit(
    turn_elapsed: Duration,
    max_wall_time: Duration,
    per_step_cap: Duration,
) -> Duration {
    if max_wall_time.is_zero() {
        per_step_cap
    } else {
        max_wall_time.saturating_sub(turn_elapsed).min(per_step_cap)
    }
}

pub(crate) fn turn_wall_clock_exhausted(turn_elapsed: Duration, max_wall_time: Duration) -> bool {
    !max_wall_time.is_zero() && turn_elapsed >= max_wall_time
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::CompactionConfig;
    use crate::config::Config;
    use crate::core::engine::{Engine, EngineConfig, UNBOUNDED_MODEL_STEPS};
    use crate::core::events::{Event, TurnOutcomeStatus};
    use crate::core::ops::{Op, UserInputProvenance};
    use crate::error_taxonomy::ErrorCategory;
    use crate::tui::app::AppMode;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    #[test]
    fn ordinary_engine_default_has_a_finite_step_and_wall_clock_budget() {
        assert_eq!(UNBOUNDED_MODEL_STEPS, u32::MAX);
        assert_eq!(DEFAULT_MAX_STEPS, 100);
        assert_eq!(DEFAULT_MAX_WALL_TIME, Duration::from_secs(30 * 60));
        assert_eq!(EngineConfig::default().max_steps, DEFAULT_MAX_STEPS);
        assert_eq!(EngineConfig::default().max_wall_time, DEFAULT_MAX_WALL_TIME);
        assert_ne!(EngineConfig::default().max_steps, UNBOUNDED_MODEL_STEPS);
    }

    #[test]
    fn stream_duration_limit_does_not_rearm_past_the_turn_budget() {
        let per_step = Duration::from_secs(1800);
        let turn_budget = Duration::from_secs(30 * 60);
        assert_eq!(
            stream_duration_limit(Duration::from_secs(20 * 60), turn_budget, per_step),
            Duration::from_secs(10 * 60),
            "a later step must inherit remaining turn time, not a fresh 30min cap"
        );
        assert_eq!(
            stream_duration_limit(Duration::ZERO, turn_budget, per_step),
            per_step
        );
        assert_eq!(
            stream_duration_limit(turn_budget, turn_budget, per_step),
            Duration::ZERO
        );
        assert_eq!(
            stream_duration_limit(Duration::from_secs(20 * 60), Duration::ZERO, per_step),
            per_step,
            "zero turn wall-clock is unbounded; only the per-step cap applies"
        );
    }

    #[test]
    fn turn_wall_clock_zero_is_unbounded() {
        assert!(!turn_wall_clock_exhausted(
            Duration::from_secs(60 * 60),
            Duration::ZERO
        ));
        assert!(turn_wall_clock_exhausted(
            Duration::from_millis(1),
            Duration::from_millis(1)
        ));
        assert!(!turn_wall_clock_exhausted(
            Duration::from_millis(1),
            Duration::from_secs(30 * 60)
        ));
    }

    fn send_op(content: &str, config: &Config) -> Op {
        Op::SendMessage {
            content: content.to_string(),
            mode: AppMode::Agent,
            route: Box::new(
                crate::route_runtime::resolve_runtime_route(
                    config,
                    config.api_provider(),
                    Some(crate::config::DEFAULT_TEXT_MODEL),
                )
                .expect("resolve test route"),
            ),
            compaction: Box::new(CompactionConfig::default()),
            goal_objective: None,
            goal_token_budget: None,
            goal_status: crate::tools::goal::GoalStatus::Active,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            allow_shell: true,
            trust_mode: false,
            auto_approve: false,
            approval_mode: crate::tui::approval::ApprovalMode::Suggest,
            translation_enabled: false,
            allowed_tools: None,
            dynamic_tools: Vec::new(),
            hook_executor: None,
            verbosity: None,
            provenance: UserInputProvenance::ExternalUser,
        }
    }

    #[tokio::test]
    async fn turn_wall_clock_exhaustion_fails_as_budget_never_completed() {
        use crate::llm_client::mock::{MockLlmClient, canned};

        let workspace = tempdir().expect("tempdir");
        let mock = Arc::new(MockLlmClient::new(vec![canned::simple_text_turn(
            "should never be requested",
        )]));
        let client: crate::core::model_client::SharedModelClient = mock.clone();
        let engine_config = EngineConfig {
            workspace: workspace.path().to_path_buf(),
            snapshots_enabled: false,
            subagents_enabled: false,
            max_wall_time: Duration::from_nanos(1),
            ..EngineConfig::default()
        };
        let (engine, handle) =
            Engine::new_with_model_client(engine_config, &Config::default(), client);
        let task = tokio::spawn(engine.run());
        handle
            .send(send_op("Keep going.", &Config::default()))
            .await
            .expect("send wall-clock trajectory");

        let mut rx = handle.rx_event.write().await;
        let (status, error) = loop {
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("timed out waiting for wall-clock trajectory")
                .expect("engine event");
            if let Event::TurnComplete { status, error, .. } = event {
                break (status, error);
            }
        };
        drop(rx);

        assert_eq!(
            status,
            TurnOutcomeStatus::Failed,
            "wall-clock exhaustion must never report Completed"
        );
        let error = error.expect("wall-clock exhaustion must carry a terminal error");
        assert!(error.contains("wall-clock budget"), "{error}");
        assert_eq!(
            mock.call_count(),
            0,
            "an already-exhausted wall-clock must not dispatch a provider request"
        );

        let category = crate::error_taxonomy::classify_error_message(&error);
        assert_eq!(category, ErrorCategory::Budget);
        assert_eq!(
            crate::core::termination::classify_turn_termination(
                status,
                Some(category),
                false,
                false
            ),
            crate::core::termination::RunTerminationReason::BudgetExhausted
        );

        handle.send(Op::Shutdown).await.expect("shutdown engine");
        task.await.expect("engine task");
    }
}
