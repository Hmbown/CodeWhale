//! `apply_*` helpers: committing an already-resolved choice to `App`, the
//! engine, and persisted settings.
//!
//! Moved verbatim out of `ui.rs`.

use super::observer_hooks::{
    execute_subagent_observer_hook, surface_observer_hook_submission_failure,
};
use super::task_projection::refresh_active_task_panel;
use super::*;

/// Record the model frozen into a child's runtime at spawn time.
///
/// This is child-route evidence, not an inference from the parent session. A
/// later usage envelope may confirm or replace it with the provider's
/// effective route while also adding provider and token facts.
pub(crate) fn record_agent_spawned_route(app: &mut App, agent_id: &str, model: &str) {
    let model =
        bound_agent_activity_text(&crate::cost_status::sanitize_persisted_route_label(model));
    app.agent_progress_meta
        .entry(agent_id.to_string())
        .or_default()
        .resolved_model = Some(model).filter(|model| !model.trim().is_empty());
}

/// Apply the normal spawn status first, then submit its observer event.
/// Submission diagnostics go to the independent toast queue, so they remain
/// visible without replacing the agent's authoritative lifecycle status.
pub(crate) fn apply_agent_spawned_status_and_observer(
    app: &mut App,
    agent_id: &str,
    prompt: &str,
    prompt_summary: &str,
) {
    let label = app.ensure_agent_label(agent_id);
    codewhale_telemetry::session_counters().bump(codewhale_telemetry::Counter::SubagentSpawn);
    app.status_message = Some(format!("{label} starting: {prompt_summary}"));
    if let Err(error) =
        execute_subagent_observer_hook(app, HookEvent::SubagentSpawn, agent_id, "prompt", prompt)
    {
        surface_observer_hook_submission_failure(app, error);
    }
}

/// Completion counterpart to [`apply_agent_spawned_status_and_observer`].
pub(crate) fn apply_agent_complete_status_and_observer(
    app: &mut App,
    agent_id: &str,
    result: &str,
    terminal_verb: &str,
) {
    let label = app.agent_display_label(agent_id);
    app.status_message = Some(format!(
        "{label} {terminal_verb}: {}",
        bound_agent_activity_text(result)
    ));
    if let Err(error) =
        execute_subagent_observer_hook(app, HookEvent::SubagentComplete, agent_id, "result", result)
    {
        surface_observer_hook_submission_failure(app, error);
    }
}

pub(crate) fn apply_coordination_detail_projection(
    app: &mut App,
    projection: crate::tools::subagent::CoordinationDetailProjection,
) {
    // §2.6: when this process does not own the workspace coordination flock,
    // say so on the sticky status strip. A silent "running (543s)" row on a
    // settled turn is a lie; surface the lock loss the same way we surface
    // other session hazards.
    //
    // Exception: a same-process handover. A model/provider switch spawns the
    // new engine before the old engine's manager has dropped the flock, and
    // flock treats the second fd in this same process as a conflict. That
    // state self-heals on the next projection retry (#5036), and a 30-second
    // warning blaming "another Codewhale process" would be false (owner
    // report, 2026-08-04) — so it stays off the sticky strip.
    if !projection.process_lock_held {
        let note = projection
            .process_lock_note
            .as_deref()
            .unwrap_or("another Codewhale process owns delegated coordination for this workspace");
        let same_process_handover =
            note.contains(crate::tools::subagent::COORDINATION_SAME_PROCESS_HANDOVER);
        // The strip is one row. The old copy opened with the diagnosis
        // ("Delegated coordination unavailable — ") and buried the cause
        // behind a `{note}` carrying a pid, an absolute workspace path, and an
        // errno, so a truncated strip showed `Delegated coordination
        // unavailable — an…` and taught the user nothing. Lead with the fact
        // that explains it — a second session is open here — and leave the pid
        // and path to the coordination detail view, which already renders
        // `process_lock_note` in full.
        let message = if note.contains(crate::tools::subagent::COORDINATION_LOCK_TIMEOUT_MARKER) {
            "Timed out claiming delegated coordination for this workspace — job rows still settle locally.".to_string()
        } else {
            "Another Codewhale session in this workspace owns delegated coordination — job rows still settle locally.".to_string()
        };
        // Demoted from sticky 30s to transient 5s — two sessions in same workspace
        // should not feel broken; job rows still settle locally. The detail view
        // still shows the full pid/path via `process_lock_note`.
        let already = app
            .status_toasts
            .iter()
            .any(|toast| toast.text.contains("delegated coordination"));
        if !already && !same_process_handover {
            app.push_status_toast(
                message,
                crate::tui::app::StatusToastLevel::Info,
                Some(5_000),
            );
        }
    }
    app.coordination_detail = Some(projection);
}

pub(crate) fn apply_alt_4_shortcut(app: &mut App, _modifiers: KeyModifiers) {
    rail_panel_shortcut(app, crate::tui::work_surface::RailPanel::Files);
}

pub(crate) fn apply_alt_0_shortcut(app: &mut App, modifiers: KeyModifiers) {
    // Ctrl+Alt+0 toggles the rail off and back to the default bottom
    // placement. Plain Alt+0 is unbound: it used to select the retired
    // auto-collapse mode.
    if modifiers.contains(KeyModifiers::CONTROL) {
        if app.work_surface.placement == crate::tui::work_surface::WorkSurfacePlacement::Off {
            app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Bottom;
            app.status_message = Some("Workbar: bottom placement".to_string());
        } else {
            app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Off;
            app.status_message = Some("Workbar is off".to_string());
        }
        app.needs_redraw = true;
    }
}

pub(crate) fn apply_picker_session_rename_to_active_app(
    app: &mut App,
    metadata: crate::session_manager::SessionMetadata,
) -> bool {
    if app.current_session_id.as_deref() != Some(metadata.id.as_str()) {
        return false;
    }
    app.session_title = Some(metadata.title.clone());
    app.current_session_metadata = Some(metadata);
    true
}

/// Translate an `EngineEvent::Error` into UI state updates.
///
/// The engine's `recoverable` flag (mirrored on `ErrorEnvelope`) decides
/// whether the session flips into offline mode: stream stalls, chunk
/// timeouts, transient network errors, and rate-limit/server hiccups arrive
/// recoverable and must NOT flip into offline. Hard failures (auth, billing,
/// invalid request) arrive non-recoverable; those flip offline so subsequent
/// messages get queued instead of silently lost mid-flight.
///
/// `severity` drives transcript color: red for `Error`/`Critical`, amber for
/// `Warning`, dim for `Info`.
pub(crate) fn apply_engine_error_to_app(
    app: &mut App,
    envelope: crate::error_taxonomy::ErrorEnvelope,
) {
    let recoverable = envelope.recoverable;
    let message = envelope.message.clone();
    let severity = envelope.severity;
    let turn_was_in_progress =
        app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"));
    streaming_thinking::finalize_current(app);
    if turn_was_in_progress {
        app.finalize_streaming_assistant_as_interrupted();
        app.finalize_active_cell_as_interrupted();
        app.runtime_turn_status = Some("failed".to_string());
    }
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #455 (observer-only): fire `on_error` hooks so operators can
    // page on auth / billing / invalid-request failures without
    // tailing the audit log. Read-only — the hook can react but not
    // suppress the error from reaching the transcript. Fast-path
    // skip when no hooks configured.
    if app
        .hooks
        .has_hooks_for_event(crate::hooks::HookEvent::OnError)
    {
        let context = app.base_hook_context().with_error(&message);
        if let Err(error) = app.submit_hooks(crate::hooks::HookEvent::OnError, context) {
            surface_observer_hook_submission_failure(app, error);
        }
    }

    app.add_message(HistoryCell::Error {
        message: message.clone(),
        severity,
    });
    app.is_loading = false;
    app.dispatch_started_at = None;
    app.turn_error_posted = true;
    if matches!(
        envelope.category,
        crate::error_taxonomy::ErrorCategory::Authentication
    ) && app.api_key_env_only
    {
        app.offline_mode = true;
        app.onboarding_needs_api_key = true;
        app.onboarding = OnboardingState::Provider;
        let provider = app.api_provider;
        let config_path = match crate::config::resolve_load_config_path(app.config_path.clone()) {
            Ok(Some(path)) => path.display().to_string(),
            Ok(None) => "~/.codewhale/config.toml".to_string(),
            Err(error) => error.to_string(),
        };
        app.status_message = Some(
            tr(app.ui_locale, MessageId::OnboardApiKeyRejectedEnv)
                .replace("{provider}", provider.as_str())
                .replace("{env}", &provider.env_vars_label())
                .replace("{path}", &config_path),
        );
        return;
    }
    if recoverable
        && matches!(
            envelope.category,
            crate::error_taxonomy::ErrorCategory::Network
                | crate::error_taxonomy::ErrorCategory::RateLimit
                | crate::error_taxonomy::ErrorCategory::Timeout
        )
        && app.advance_fallback(message.clone()).is_some()
    {
        let position = app.fallback_chain_position().unwrap_or(0);
        let total = app.fallback_chain_len();
        app.status_message = Some(format!(
            "Switched to {} (fallback {position}/{}) after recoverable provider error.",
            app.api_provider.as_str(),
            total.saturating_sub(1)
        ));
        return;
    }
    if !recoverable {
        app.offline_mode = true;
    }
    // Error is already in the transcript as HistoryCell::Error above;
    // don't emit a redundant status_message that would become a sticky
    // toast in the footer — that duplicates the transcript entry.
}

/// Apply the gate result on the event loop. Returns `true` when dispatch may
/// continue; a denial leaves the original message out of history/model input.
pub(crate) fn apply_message_submit_outcome(
    app: &mut App,
    message: &mut QueuedMessage,
    outcome: crate::hooks::MessageSubmitOutcome,
) -> bool {
    if let Some(warning) = outcome.warning() {
        app.status_message = Some(warning.to_string());
    }
    match outcome {
        crate::hooks::MessageSubmitOutcome::Unchanged { .. } => true,
        crate::hooks::MessageSubmitOutcome::Replaced { text, .. } => {
            message.display = text;
            true
        }
        crate::hooks::MessageSubmitOutcome::Blocked { reason } => {
            app.status_message = Some(reason);
            false
        }
    }
}

fn visible_goal_as_durable(
    app: &App,
) -> Result<Option<crate::session_manager::SessionGoalState>, String> {
    let Some(objective) = app.goal.objective.as_deref() else {
        return Ok(None);
    };
    let elapsed_seconds = app
        .goal
        .started_at
        .map(|started| started.elapsed().as_secs())
        .unwrap_or(app.goal.time_used_seconds)
        .max(app.goal.time_used_seconds);
    crate::session_manager::SessionGoalState::from_runtime(&GoalSnapshot {
        objective: Some(objective.to_string()),
        status: app.goal.status.as_str().to_string(),
        token_budget: app.goal.token_budget,
        tokens_used: app.goal.tokens_used,
        time_used_seconds: app.goal.time_used_seconds,
        continuation_count: app.goal.continuation_count,
        elapsed_seconds: Some(elapsed_seconds),
        pause_reason: app.goal.pause_reason,
        ..Default::default()
    })
    .map_err(|error| error.to_string())
}

fn desired_goal_state(
    app: &App,
    intent: &GoalControlIntent,
) -> Result<Option<crate::session_manager::SessionGoalState>, String> {
    let mut base = if app.pending_goal_controls.is_empty() {
        match app.last_known_goal_state.clone() {
            Some(goal) => Some(goal),
            None => visible_goal_as_durable(app)?,
        }
    } else {
        // Accepted controls compose over the latest durable target, not the
        // older visible projection that is still waiting on GoalUpdated.
        app.last_known_goal_state.clone()
    };
    match intent {
        GoalControlIntent::SetStatus { clear: true, .. } => Ok(None),
        GoalControlIntent::SetStatus {
            status,
            clear: false,
        } => {
            let goal = base
                .as_mut()
                .ok_or_else(|| "No goal is available for this control.".to_string())?;
            goal.status = match status {
                GoalStatus::Active => crate::session_manager::SessionGoalStatus::Active,
                GoalStatus::Paused => crate::session_manager::SessionGoalStatus::Paused,
                GoalStatus::Complete => crate::session_manager::SessionGoalStatus::Complete,
                GoalStatus::Blocked => crate::session_manager::SessionGoalStatus::Blocked,
            };
            goal.pause_reason = (*status == GoalStatus::Paused)
                .then_some(crate::tools::goal::GoalPauseReason::User);
            Ok(base)
        }
        GoalControlIntent::SetObjective {
            objective,
            token_budget,
        } => crate::session_manager::SessionGoalState::from_runtime(&GoalSnapshot {
            objective: Some(objective.clone()),
            status: GoalStatus::Active.as_str().to_string(),
            token_budget: *token_budget,
            elapsed_seconds: Some(0),
            ..Default::default()
        })
        .map_err(|error| error.to_string()),
    }
}

fn persist_accepted_goal_state(
    app: &mut App,
    desired: Option<&crate::session_manager::SessionGoalState>,
) -> Result<(), String> {
    let manager = SessionManager::default_location()
        .map_err(|error| format!("could not open the session store: {error}"))?;
    if app.current_session_id.is_none() {
        let session = build_session_snapshot(app, &manager)?;
        let session_id = session.metadata.id.clone();
        if !persistence_actor::try_persist(PersistRequest::SaveCheckpoint { session }) {
            return Err("the persistence worker is unavailable".to_string());
        }
        app.current_session_id = Some(session_id);
    }
    let session_id = app
        .current_session_id
        .as_deref()
        .ok_or_else(|| "session id is not established".to_string())?;
    manager
        .save_session_goal(session_id, desired)
        .map_err(|error| error.to_string())
}

fn goal_control_op(intent: &GoalControlIntent) -> Op {
    match intent {
        GoalControlIntent::SetStatus { status, clear } => Op::SetGoalStatus {
            status: *status,
            clear: *clear,
        },
        GoalControlIntent::SetObjective {
            objective,
            token_budget,
        } => Op::SetGoalObjective {
            objective: objective.clone(),
            token_budget: *token_budget,
        },
    }
}

/// Retry accepted goal controls without ever awaiting mailbox capacity on the
/// input loop. FIFO order is retained until each authoritative receipt lands.
pub(crate) fn flush_pending_goal_controls(app: &mut App, engine_handle: &EngineHandle) -> bool {
    for pending in &mut app.pending_goal_controls {
        if pending.dispatched {
            continue;
        }
        if engine_handle
            .try_send(goal_control_op(&pending.intent))
            .is_err()
        {
            return engine_handle.tx_op.is_closed();
        }
        pending.dispatched = true;
    }
    false
}

fn goal_control_matches(
    intent: &GoalControlIntent,
    durable: Option<&crate::session_manager::SessionGoalState>,
) -> bool {
    match intent {
        GoalControlIntent::SetStatus { clear: true, .. } => durable.is_none(),
        GoalControlIntent::SetStatus {
            status,
            clear: false,
        } => durable.is_some_and(|goal| {
            goal.status
                == match status {
                    GoalStatus::Active => crate::session_manager::SessionGoalStatus::Active,
                    GoalStatus::Paused => crate::session_manager::SessionGoalStatus::Paused,
                    GoalStatus::Complete => crate::session_manager::SessionGoalStatus::Complete,
                    GoalStatus::Blocked => crate::session_manager::SessionGoalStatus::Blocked,
                }
        }),
        GoalControlIntent::SetObjective {
            objective,
            token_budget,
        } => durable.is_some_and(|goal| {
            goal.objective == *objective
                && goal.status == crate::session_manager::SessionGoalStatus::Active
                && goal.token_budget == *token_budget
        }),
    }
}

fn accept_goal_control(app: &mut App, engine_handle: &EngineHandle, intent: GoalControlIntent) {
    let desired = match desired_goal_state(app, &intent) {
        Ok(desired) => desired,
        Err(error) => {
            surface_goal_persistence_failure(app, &error);
            return;
        }
    };
    if let Err(error) = persist_accepted_goal_state(app, desired.as_ref()) {
        surface_goal_persistence_failure(app, &error);
        return;
    }

    if matches!(
        intent,
        GoalControlIntent::SetStatus {
            status: GoalStatus::Complete,
            clear: false
        }
    ) {
        crate::audit::log_sensitive_event(
            "goal.user_completed",
            serde_json::json!({ "accepted": true }),
        );
    }
    app.last_known_goal_state = desired;
    app.pending_goal_controls.push_back(PendingGoalControl {
        intent,
        dispatched: false,
    });
    let runtime_closed = flush_pending_goal_controls(app, engine_handle);
    app.add_message(HistoryCell::System {
        content: app.tr(MessageId::GoalControlAccepted).to_string(),
    });
    if runtime_closed {
        app.push_status_toast(
            app.tr(MessageId::GoalControlRuntimeUnavailable).to_string(),
            StatusToastLevel::Warning,
            None,
        );
    }
}

pub(crate) fn apply_goal_snapshot_to_app(app: &mut App, snapshot: &GoalSnapshot) -> bool {
    let durable_goal = match crate::session_manager::SessionGoalState::from_runtime(snapshot) {
        Ok(goal) => goal,
        Err(error) => {
            tracing::warn!("ignoring invalid runtime goal snapshot: {error}");
            return false;
        }
    };
    let pending_desired = app.last_known_goal_state.clone();
    let matched_pending = app
        .pending_goal_controls
        .front()
        .is_some_and(|pending| goal_control_matches(&pending.intent, durable_goal.as_ref()));
    if matched_pending {
        app.pending_goal_controls.pop_front();
    }
    // An explicit engine-side clear is represented by the one canonical empty
    // state emitted by GoalState::snapshot. Require both fields so a malformed
    // objective-less Active/Blocked update cannot erase valid visible state.
    if snapshot.objective.is_none() && snapshot.status.trim() == "none" {
        let changed = app.goal.objective.is_some()
            || app.goal.token_budget.is_some()
            || app.goal.tokens_used != 0
            || app.goal.time_used_seconds != 0
            || app.goal.continuation_count != 0
            || app.goal.started_at.is_some()
            || app.goal.finished_at.is_some()
            || app.goal.status != GoalStatus::default();
        app.goal = crate::tui::app::HostGoalState::default();
        app.last_known_goal_state = if app.pending_goal_controls.is_empty() {
            None
        } else {
            pending_desired
        };
        return changed || matched_pending;
    }

    let Some(objective) = snapshot
        .objective
        .as_deref()
        .map(str::trim)
        .filter(|objective| !objective.is_empty())
    else {
        tracing::warn!(
            "ignoring objective-less runtime goal snapshot with non-clear status: {}",
            snapshot.status
        );
        return false;
    };
    let Some(status) = goal_status_from_snapshot(snapshot) else {
        tracing::warn!("ignoring unknown runtime goal status: {}", snapshot.status);
        return false;
    };
    let verdict = status;
    let objective_changed = app.goal.objective.as_deref() != Some(objective);
    let changed = objective_changed
        || app.goal.token_budget != snapshot.token_budget
        || app.goal.tokens_used != snapshot.tokens_used
        || app.goal.time_used_seconds != snapshot.time_used_seconds
        || app.goal.continuation_count != snapshot.continuation_count
        || app.goal.pause_reason != snapshot.pause_reason
        || app.goal.status != verdict;
    if !changed {
        app.last_known_goal_state = if app.pending_goal_controls.is_empty() {
            durable_goal
        } else {
            pending_desired
        };
        return matched_pending;
    }

    // The runtime introduced a new active objective (the model called
    // `create_goal`, or a restored session carried one): say so once, in one
    // line, so the user knows a persistent goal is now driving turns and how
    // to stop it. `/goal <objective>` sets the objective before this snapshot lands,
    // so a user-declared goal does not repeat its own receipt.
    if objective_changed && verdict == GoalStatus::Active {
        // Operate set it from the prompt (or the model did while operating);
        // the objective is the prompt the user just typed, so the receipt
        // says what Operate will do with it instead of echoing it.
        let content = if app.mode == crate::tui::app::AppMode::Operate {
            app.tr(crate::localization::MessageId::GoalReceiptSetOperate)
                .into_owned()
        } else {
            app.tr(crate::localization::MessageId::GoalReceiptSet)
                .replace("{objective}", objective)
        };
        app.add_message(crate::tui::history::HistoryCell::System { content });
    }
    app.goal.objective = Some(objective.to_string());
    app.goal.token_budget = snapshot.token_budget;
    app.goal.tokens_used = snapshot.tokens_used;
    app.goal.time_used_seconds = snapshot.time_used_seconds;
    app.goal.continuation_count = snapshot.continuation_count;
    app.goal.pause_reason = snapshot.pause_reason;
    app.goal.status = verdict;
    if objective_changed || app.goal.started_at.is_none() {
        let now = Instant::now();
        let elapsed = std::time::Duration::from_secs(snapshot.elapsed_seconds.unwrap_or_default());
        app.goal.started_at = now.checked_sub(elapsed).or(Some(now));
    }
    // Freeze the elapsed timer the first time a goal leaves the active state.
    // Paused (Wounded) goals freeze too — usage snapshots keep arriving while
    // paused, and clearing here would silently un-freeze a timer the user just
    // paused (matching close_hunt, which records the pause instant). Only an
    // explicit resume back to Hunting re-arms the timer.
    match verdict {
        GoalStatus::Complete | GoalStatus::Blocked | GoalStatus::Paused => {
            if app.goal.finished_at.is_none() {
                app.goal.finished_at = Some(Instant::now());
            }
        }
        GoalStatus::Active => app.goal.finished_at = None,
    }
    app.last_known_goal_state = if app.pending_goal_controls.is_empty() {
        durable_goal
    } else {
        pending_desired
    };
    true
}

/// Apply an explicit mode selection from a user shortcut (Alt+A/P/Y).
///
/// Uses `select_mode`, not `set_mode`, so an explicitly chosen mode is also the
/// startup default next launch – matching the Tab cycle and hotbar paths.
pub(crate) async fn apply_mode_update(
    app: &mut App,
    engine_handle: &EngineHandle,
    config: &Config,
    mode: AppMode,
) -> bool {
    let outcome = app.select_mode(mode);
    app.report_mode_selection(mode, outcome);
    if mode == AppMode::Operate {
        present_operate_board(app, config).await;
    }
    if outcome.changed_live_state() {
        sync_mode_update(app, engine_handle).await;
        true
    } else {
        false
    }
}

/// Apply the legacy YOLO shortcut (Alt+Y): a permission change, not a mode
/// change. Same persist/report/sync contract as [`apply_mode_update`]; the
/// startup default written is the mode actually installed (Act).
pub(crate) async fn apply_yolo_compat_update(
    app: &mut App,
    engine_handle: &EngineHandle,
    _config: &Config,
) -> bool {
    let outcome = app.select_yolo_compat();
    app.report_mode_selection(AppMode::Agent, outcome);
    if outcome.changed_live_state() {
        sync_mode_update(app, engine_handle).await;
        true
    } else {
        false
    }
}

/// Entering Operate attaches to the recorded operation (a fresh one only
/// when none exists or the last was cancelled), shows the localized lead
/// plan, and keeps always-on mode durable by reinstalling the hourly lead
/// keepalive bound to this workspace. Burn rate is optional; default is
/// unbounded.
async fn present_operate_board(app: &mut App, config: &Config) {
    let store = match crate::operate::OperationStore::open(crate::operate::default_operate_dir()) {
        Ok(store) => store,
        Err(error) => {
            app.add_message(crate::tui::history::HistoryCell::System {
                content: format!("Operate store unavailable: {error}"),
            });
            return;
        }
    };
    let credentials = crate::operate::operate_credentials_present(config);
    let operation = match crate::operate::attach_or_start_operation(
        &store,
        &app.workspace,
        None,
        None,
        credentials,
    ) {
        Ok(mut operation) => {
            if credentials && !operation.direction.is_empty() && operation.lead_plan.is_none() {
                operation.plan_from_direction();
                if let Err(error) = store.save(&operation) {
                    app.add_message(crate::tui::history::HistoryCell::System {
                        content: format!("Operate plan not saved: {error}"),
                    });
                }
            }
            operation
        }
        Err(error) => {
            app.add_message(crate::tui::history::HistoryCell::System {
                content: format!("Operate did not start: {error}"),
            });
            return;
        }
    };
    // Always-on is durable only if the keepalive automation exists: entering
    // Operate (re)installs it for this workspace, kicking an immediate
    // lead-plan step when the attached operation still needs one.
    let needs_lead_plan = !operation
        .lead_plan
        .as_ref()
        .is_some_and(|plan| !plan.slices.is_empty());
    if let Some(automations) = app
        .runtime_services
        .automations
        .as_ref()
        .map(std::sync::Arc::clone)
    {
        let manager = automations.lock().await;
        if let Err(error) =
            crate::operate::upsert_keepalive(&manager, &app.workspace, needs_lead_plan)
        {
            app.add_message(crate::tui::history::HistoryCell::System {
                content: format!("Operate keep-alive not installed: {error}"),
            });
        }
    }
    app.add_message(crate::tui::history::HistoryCell::System {
        content: crate::operate::render_plan_board_locale(&operation, app.ui_locale),
    });
}

pub(crate) async fn apply_model_and_compaction_update(
    engine_handle: &EngineHandle,
    compaction: crate::compaction::CompactionConfig,
    mode: AppMode,
    route_limits: Option<codewhale_config::route::RouteLimits>,
) {
    let _ = engine_handle
        .send(Op::SetModel {
            model: compaction.model.clone(),
            mode,
            route_limits,
        })
        .await;
    let _ = engine_handle
        .send(Op::SetCompaction { config: compaction })
        .await;
}

/// Apply the choice made in the `/model` picker (#39): mutate App state so
/// the next turn uses the new model/effort, push the change to the running
/// engine via `Op::SetModel`/`Op::SetCompaction`, and surface a one-line
/// status describing what changed. Startup persistence is intentionally owned
/// by the picker's explicit Shift+D action in the view-event handler.
// The model/effort transition needs both the previous and next model+effort
// plus the engine, app, and config handles; bundling them into a struct here
// would only obscure a straightforward orchestration step.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_model_picker_choice(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    model: String,
    target_provider: Option<ApiProvider>,
    target_provider_id: Option<String>,
    effort: crate::tui::app::ReasoningEffort,
    previous_model: String,
    previous_effort: crate::tui::app::ReasoningEffort,
    save_as_startup_default: bool,
) {
    if app.reject_setting_change_while_busy(
        crate::localization::MessageId::SettingSubjectModelAndThinking,
    ) {
        note_startup_default_not_saved(app, save_as_startup_default);
        return;
    }
    let target_provider = target_provider.unwrap_or(app.api_provider);
    let target_identity = if target_provider == ApiProvider::Custom {
        target_provider_id.unwrap_or_else(|| config.provider_identity_for(target_provider))
    } else {
        target_provider.as_str().to_string()
    };
    let model_is_auto = model.trim().eq_ignore_ascii_case("auto");
    let preserve_auto_effort =
        app.reasoning_effort_preference.is_some() || effort != previous_effort;
    if target_provider != app.api_provider
        || target_identity != app.provider_identity_for_persistence()
    {
        config.provider = Some(target_identity.clone());
        switch_provider(
            app,
            engine_handle,
            config,
            target_provider,
            (!model_is_auto).then_some(model.clone()),
        )
        .await;
        if app.api_provider != target_provider
            || app.provider_identity_for_persistence() != target_identity
        {
            // The switch was refused (missing credentials, bad route). The
            // live route is still the old one, so persisting it as the startup
            // default would silently pin the route the user just tried to leave.
            note_startup_default_not_saved(app, save_as_startup_default);
            return;
        }
        if !model_is_auto {
            apply_picker_effort_choice(app, engine_handle, effort, previous_effort).await;
            app.fleet_roster_stale |= crate::fleet::members::auto_enroll_fleet_model(
                &app.workspace,
                app.provider_identity_for_persistence(),
                &app.model,
            );
            if save_as_startup_default {
                app.status_message = Some(app.save_live_route_as_startup_default());
            }
            return;
        }
    }

    let model_changed = model != previous_model || app.auto_model != model_is_auto;
    let mut resolved_model = model.clone();
    let mut route_base_url = config.deepseek_base_url();
    if !model_is_auto {
        let saved_provider_model = config
            .provider_config_for(app.api_provider)
            .and_then(|provider| provider.model.as_deref());
        match crate::route_runtime::resolve_route_candidate_with_context_metadata(
            app.api_provider,
            Some(&model),
            saved_provider_model,
            Some(config.deepseek_base_url()),
            config.context_window_for_provider_config(app.api_provider),
            None,
        ) {
            Ok(resolution) => {
                resolved_model = resolution.candidate.wire_model_id().as_str().to_string();
                route_base_url = resolution.candidate.endpoint().base_url.clone();
                if model_changed {
                    app.set_active_context_window_override(
                        config.context_window_for_provider_config(app.api_provider),
                    );
                    app.set_active_route_resolution(
                        route_base_url.clone(),
                        resolution.candidate.limits(),
                        resolution.context_window.source,
                    );
                }
            }
            Err(reason) => {
                app.status_message = Some(reason);
                note_startup_default_not_saved(app, save_as_startup_default);
                return;
            }
        }
    } else if model_changed {
        app.set_active_context_window_override(
            config.context_window_for_provider_config(app.api_provider),
        );
        app.active_route_limits = app.context_window_override_limits();
        app.active_route_base_url = route_base_url.clone();
        app.active_context_window_source = if app.active_context_window_override.is_some() {
            crate::route_runtime::ContextWindowSource::Configured
        } else {
            crate::route_runtime::ContextWindowSource::Fallback
        };
    }

    let effective_effort = if model_is_auto {
        effort
    } else {
        effort.normalize_for_route(app.api_provider, &route_base_url, &resolved_model)
    };
    let effort_changed = effort != previous_effort;

    if model_changed {
        app.set_model_selection(resolved_model.clone());
        let provider_identity = app.provider_identity_for_persistence().to_string();
        app.provider_models
            .insert(provider_identity.clone(), resolved_model.clone());
        app.enable_provider_model(&provider_identity, &resolved_model);
        app.clear_model_scoped_telemetry();
    }
    let preference_changed = if model_is_auto && !preserve_auto_effort {
        app.reasoning_effort_preference.take().is_some()
    } else {
        let changed = app.reasoning_effort_preference != Some(effort);
        app.reasoning_effort_preference = Some(effort);
        changed
    };
    let live_effort_changed = effective_effort != app.reasoning_effort;
    if !model_is_auto || preserve_auto_effort {
        app.reasoning_effort = effective_effort;
    } else {
        app.reasoning_effort = ReasoningEffort::Auto;
    }
    if live_effort_changed || preference_changed {
        app.invalidate_route_receipts_for_reasoning_change();
    }
    if model_changed || live_effort_changed || preference_changed {
        app.update_model_compaction_budget();
    }

    // A model pick is session-local by default. Keep the exact live route in
    // memory and offer an explicit save decision; only Shift+D in the picker
    // writes a startup default.
    let route_provider = app.provider_identity_for_persistence().to_string();
    app.note_session_route_change(&route_provider, &resolved_model);
    if !model_is_auto {
        app.fleet_roster_stale |= crate::fleet::members::auto_enroll_fleet_model(
            &app.workspace,
            &route_provider,
            &resolved_model,
        );
    }

    if model_changed {
        apply_model_and_compaction_update(
            engine_handle,
            app.compaction_config(),
            app.mode,
            app.active_route_limits,
        )
        .await;
    }

    let model_summary = if model_is_auto {
        "auto (per-turn model)".to_string()
    } else {
        resolved_model.clone()
    };
    let previous_effort_summary = previous_effort.display_label_for_provider(app.api_provider);
    let applied_effort = app.reasoning_effort;
    let effort_summary = if applied_effort == ReasoningEffort::Auto {
        "auto (per-turn thinking)".to_string()
    } else {
        applied_effort
            .display_label_for_provider(app.api_provider)
            .to_string()
    };

    let summary = match (model_changed, effort_changed) {
        (true, true) => format!(
            "Model: {previous_model} → {model_summary} · thinking: {previous_effort_summary} → {effort_summary}"
        ),
        (true, false) => {
            format!("Model: {previous_model} → {model_summary} · thinking {effort_summary}")
        }
        (false, true) => format!(
            "Thinking: {previous_effort_summary} → {effort_summary} · model {model_summary}"
        ),
        (false, false) => {
            format!("Model unchanged: {model_summary} · thinking {effort_summary}")
        }
    };
    app.status_message = Some(summary);
    // Setup progress records that a concrete route was selected successfully;
    // it is a local receipt, not a claim that the route became the default.
    if model_changed || !model_is_auto {
        record_provider_model_setup_progress(app, config);
    }
    if save_as_startup_default {
        app.status_message = Some(app.save_live_route_as_startup_default());
    }
}

pub(crate) async fn apply_picker_effort_choice(
    app: &mut App,
    engine_handle: &EngineHandle,
    effort: ReasoningEffort,
    previous_effort: ReasoningEffort,
) {
    if app.reject_setting_change_while_busy(crate::localization::MessageId::SettingSubjectThinking)
    {
        return;
    }
    let effective_effort = if app.auto_model {
        effort
    } else {
        effort.normalize_for_route(app.api_provider, &app.active_route_base_url, &app.model)
    };
    let live_changed = effective_effort != app.reasoning_effort;
    let preference_changed = app.reasoning_effort_preference != Some(effort);
    let selection_changed = effort != previous_effort || live_changed;

    if live_changed || preference_changed {
        app.reasoning_effort = effective_effort;
        app.reasoning_effort_preference = Some(effort);
    }
    if selection_changed {
        app.invalidate_route_receipts_for_reasoning_change();
        app.update_model_compaction_budget();
    }

    let persist_warning = app
        .startup_defaults
        .apply_blocking(
            crate::tui::startup_defaults::StartupDefaults::reasoning_effort(effort.as_setting()),
        )
        .err()
        .map(|err| format!(" (not persisted: {err})"));

    if live_changed {
        apply_model_and_compaction_update(
            engine_handle,
            app.compaction_config(),
            app.mode,
            app.active_route_limits,
        )
        .await;
    }

    let persisted = persist_warning.is_none();
    let mut summary = if selection_changed {
        format!(
            "Thinking: {} → {} · model {}",
            previous_effort.display_label_for_provider(app.api_provider),
            effort.display_label_for_provider(app.api_provider),
            app.model_display_label()
        )
    } else {
        let mut summary = format!(
            "Thinking unchanged: {} · model {}",
            effort.display_label_for_provider(app.api_provider),
            app.model_display_label()
        );
        if persisted {
            summary.push_str(" · ");
            summary.push_str(&app.tr(crate::localization::MessageId::SavedAsStartupDefault));
        }
        summary
    };
    if let Some(warning) = persist_warning {
        summary.push_str(&warning);
    }
    app.status_message = Some(summary);
}

pub(crate) async fn apply_provider_fallback_switch(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    rollback: ProviderFallbackRollback,
) {
    let ProviderFallbackRollback {
        identity: previous_identity,
        chain: previous_chain,
    } = rollback;
    let previous_provider = previous_identity.provider;
    let target = app.api_provider;
    let previous_model = app.model.clone();

    let resolved_route = match resolve_runtime_route(config, target, None) {
        Ok(route) => route,
        Err(reason) => {
            app.set_provider_identity_record(previous_identity.clone());
            app.provider_chain = previous_chain.clone();
            app.last_fallback_reason = Some(format!(
                "Fallback provider {} route was rejected: {reason}",
                target.as_str()
            ));
            app.status_message = Some(format!(
                "Fallback provider {} rejected; provider remains {}.",
                target.as_str(),
                previous_provider.as_str()
            ));
            return;
        }
    };
    let target_identity = resolved_route.identity.clone();
    let resolved_endpoint = resolved_route.candidate.endpoint().base_url.clone();
    let next_config = resolved_route.config;
    let new_model = resolved_route.model;
    let context_window_source = resolved_route.context_window.source;

    if let Err(err) = DeepSeekClient::from_candidate(&next_config, &resolved_route.candidate) {
        app.set_provider_identity_record(previous_identity);
        app.provider_chain = previous_chain;
        app.last_fallback_reason = Some(format!(
            "Fallback provider {} was unavailable: {err}",
            target.as_str()
        ));
        app.status_message = Some(format!(
            "Fallback provider {} unavailable; provider remains {}.",
            target.as_str(),
            previous_provider.as_str()
        ));
        return;
    }
    *config = *next_config;
    app.set_provider_identity_record(target_identity);
    app.billing_presentation = crate::route_billing::for_route(config, target);

    let new_base_url = resolved_endpoint;
    let new_endpoint = display_base_url_host(&new_base_url);
    let cache_scope_changed = previous_provider != target || previous_model != new_model;
    app.model_ids_passthrough = config.model_ids_pass_through();
    app.set_model_selection(new_model.clone());
    app.apply_provider_switch_reasoning_effort(target, &new_base_url, None);
    app.set_active_context_window_override(config.context_window_for_provider_config(target));
    app.set_active_route_resolution(
        new_base_url.clone(),
        resolved_route.candidate.limits(),
        context_window_source,
    );
    app.update_model_compaction_budget();
    if cache_scope_changed {
        app.clear_model_scoped_telemetry();
    } else {
        app.session.last_prompt_tokens = None;
        app.session.last_completion_tokens = None;
        app.session.last_output_throughput = None;
    }

    let _ = engine_handle.send(Op::Shutdown).await;
    let engine_config = build_engine_config(app, config);
    *engine_handle = spawn_tui_engine(engine_config, config);

    if !app.api_messages.is_empty() {
        let _ = engine_handle
            .send(Op::SyncSession {
                session_id: app.current_session_id.clone(),
                messages: app.api_messages.clone(),
                system_prompt: app.system_prompt.clone(),
                system_prompt_override: false,
                model: app.model.clone(),
                workspace: app.workspace.clone(),
                mode: app.mode,
            })
            .await;
    }
    let _ = engine_handle
        .send(Op::SetCompaction {
            config: app.compaction_config(),
        })
        .await;

    app.add_message(HistoryCell::System {
        content: format!(
            "Provider fallback: {} -> {}\nModel: {} -> {}\nEndpoint: {}",
            previous_provider.as_str(),
            target.as_str(),
            previous_model,
            new_model,
            new_endpoint
        ),
    });
    app.status_message = Some(format!(
        "Fallback provider: {} via {}",
        target.as_str(),
        new_endpoint
    ));
}

pub(super) fn reject_inline_inference_while_runtime_chat_owns_run(
    app: &mut App,
    result: &commands::CommandResult,
) -> bool {
    let blocked_inline_inference = matches!(
        result.action.as_ref(),
        Some(AppAction::VoiceCapture | AppAction::CacheWarmup)
    ) && app.remote_control.runtime_chat_blocks_local_dispatch();
    if !blocked_inline_inference {
        return false;
    }
    if matches!(result.action.as_ref(), Some(AppAction::VoiceCapture)) {
        // `/voice` toggles this before returning the action. Restore the state
        // so a retry after relay settlement starts capture rather than merely
        // toggling the stale flag off.
        app.voice_enabled = false;
    }
    let notice = app
        .tr(MessageId::SettingLockedDuringTurn)
        .replace("{setting}", "Codewhale Runtime");
    app.push_status_toast(notice, crate::tui::app::StatusToastLevel::Info, Some(6_000));
    true
}

pub(crate) async fn apply_command_result(
    terminal: &mut AppTerminal,
    app: &mut App,
    engine_handle: &mut EngineHandle,
    task_manager: &SharedTaskManager,
    config: &mut Config,
    #[cfg_attr(not(feature = "web"), allow(unused_variables))] web_config_session: &mut Option<
        WebConfigSession,
    >,
    result: commands::CommandResult,
) -> Result<bool> {
    // These two actions await participant inference inline on the UI event
    // loop. Waiting behind Runtime Chat's exclusive writer here would
    // deadlock: this same loop must drain the terminal projection/server
    // cursor that releases the writer. Fail closed before displaying the
    // command's optimistic message or invoking recorder/provider code.
    if reject_inline_inference_while_runtime_chat_owns_run(app, &result) {
        return Ok(false);
    }
    if let Some(msg) = result.message {
        app.add_message(HistoryCell::System { content: msg });
    }

    if let Some(action) = result.action {
        match action {
            AppAction::Quit => {
                let _ = engine_handle.send(Op::Shutdown).await;
                return Ok(true);
            }
            AppAction::LoadSession(path) => {
                let session: SavedSession = match std::fs::read_to_string(&path)
                    .map_err(|err| err.to_string())
                    .and_then(|raw| serde_json::from_str(&raw).map_err(|err| err.to_string()))
                {
                    Ok(session) => session,
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Failed to load session from {}: {err}",
                            path.display()
                        ));
                        return Ok(false);
                    }
                };
                let fresh_config =
                    match Config::load(app.config_path.clone(), app.config_profile.as_deref()) {
                        Ok(config) => config,
                        Err(err) => {
                            app.status_message = Some(format!(
                                "Failed to load live config for session restore: {err}"
                            ));
                            return Ok(false);
                        }
                    };
                let respawn = match apply_loaded_session_config_snapshot(
                    app,
                    config,
                    &session,
                    fresh_config,
                    true,
                ) {
                    Ok(outcome) => outcome,
                    Err(err) => {
                        app.status_message = Some(format!("Failed to restore session: {err}"));
                        return Ok(false);
                    }
                };
                sync_runtime_workspace_state(task_manager, app.workspace.clone()).await;
                if respawn {
                    let _ = engine_handle.send(Op::Shutdown).await;
                    *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                } else {
                    let _ = engine_handle
                        .send(Op::SetModel {
                            model: app.model.clone(),
                            mode: app.mode,
                            route_limits: app.active_route_limits,
                        })
                        .await;
                }
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id: app.current_session_id.clone(),
                        messages: app.api_messages.clone(),
                        system_prompt: app.system_prompt.clone(),
                        system_prompt_override: false,
                        model: app.model.clone(),
                        workspace: app.workspace.clone(),
                        mode: app.mode,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SetCompaction {
                        config: app.compaction_config(),
                    })
                    .await;
                let success_message = format!(
                    "Session loaded from {} (ID: {}, {} messages)",
                    path.display(),
                    crate::session_manager::truncate_id(&session.metadata.id),
                    session.metadata.message_count
                );
                app.add_message(HistoryCell::System {
                    content: success_message.clone(),
                });
                app.status_message = Some(success_message);
            }
            AppAction::SyncSession {
                session_id,
                messages,
                system_prompt,
                model,
                workspace,
                mode,
            } => {
                let mut session_id = session_id;
                let is_full_reset = messages.is_empty() && system_prompt.is_none();
                if is_full_reset && session_id.is_none() {
                    let new_session_id = uuid::Uuid::new_v4().to_string();
                    app.current_session_id = Some(new_session_id.clone());
                    session_id = Some(new_session_id);
                }
                let workspace_changed = task_manager.default_workspace().await != workspace;
                if workspace_changed {
                    apply_workspace_runtime_state(app, config, workspace.clone());
                    sync_runtime_workspace_state(task_manager, workspace.clone()).await;
                }
                let provider_changed = config.api_provider() != app.api_provider
                    || config.provider_identity_for(config.api_provider())
                        != app.provider_identity_for_persistence();
                if provider_changed {
                    let identity = match config
                        .resolve_provider_identity(app.provider_identity_for_persistence())
                    {
                        Ok(identity) => identity,
                        Err(err) => {
                            app.status_message =
                                Some(format!("Failed to restore saved session provider: {err}"));
                            return Ok(false);
                        }
                    };
                    restore_loaded_session_provider(app, config, identity);
                    config.set_provider_model_override(app.api_provider, Some(model.clone()));
                }
                // Re-resolve from the live config even when the provider did
                // not change. The command layer intentionally has no Config
                // handle, so its provisional limits cannot include current
                // provider overrides.
                resolve_loaded_session_route(app, config);
                app.update_model_compaction_budget();
                if provider_changed || workspace_changed {
                    let _ = engine_handle.send(Op::Shutdown).await;
                    *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                }
                // SyncSession carries the conversation but not resolved route
                // limits. Refresh the engine's model first so a loaded,
                // forked, or freshly reset session cannot retain the previous
                // route's context/output facts.
                let _ = engine_handle
                    .send(Op::SetModel {
                        model: model.clone(),
                        mode,
                        route_limits: app.active_route_limits,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SyncSession {
                        session_id,
                        messages,
                        system_prompt,
                        system_prompt_override: false,
                        model,
                        workspace,
                        mode,
                    })
                    .await;
                let _ = engine_handle
                    .send(Op::SetCompaction {
                        config: app.compaction_config(),
                    })
                    .await;
                if is_full_reset {
                    persist_full_reset_snapshot(app);
                }
            }
            AppAction::ModeChanged(_mode) => {
                sync_mode_update(app, engine_handle).await;
            }
            AppAction::ApprovalPolicyPersisted { policy } => {
                config.approval_policy = policy;
                sync_mode_update(app, engine_handle).await;
            }
            AppAction::PermissionRulesChanged => {
                match codewhale_config::load_permissions_snapshot(app.config_path.clone()) {
                    Ok(snapshot) => {
                        let ruleset = snapshot.permissions().ruleset();
                        config.exec_policy_engine.set_ruleset(ruleset.clone());
                        if let Err(error) = engine_handle
                            .send(Op::SetPermissionRuleset { ruleset })
                            .await
                        {
                            app.status_message = Some(
                                tr(app.ui_locale, MessageId::PermissionsOperationFailed)
                                    .replace("{error}", &error.to_string()),
                            );
                        }
                    }
                    Err(error) => {
                        app.status_message = Some(
                            tr(app.ui_locale, MessageId::PermissionsOperationFailed)
                                .replace("{error}", &format!("{error:#}")),
                        );
                    }
                }
            }
            AppAction::PluginRegistryChanged => {
                let command_errors = crate::commands::user_registry::install_plugin_registry(
                    &app.workspace,
                    app.plugin_registry.as_ref(),
                );
                app.hooks = app.hooks.rebind(
                    crate::hooks::HooksConfig::load_with_project_and_plugins(
                        config.hooks_config(),
                        &app.workspace,
                        Some(app.plugin_registry.as_ref()),
                    ),
                    app.workspace.clone(),
                );
                app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
                if !command_errors.is_empty() {
                    app.set_sticky_status(
                        format!(
                            "Plugin runtime activation failed: {}",
                            command_errors.join("; ")
                        ),
                        StatusToastLevel::Error,
                        None,
                    );
                }
                let _ = engine_handle.send(Op::Shutdown).await;
                *engine_handle = spawn_tui_engine(build_engine_config(app, config), config);
                if !app.api_messages.is_empty() {
                    let _ = engine_handle
                        .send(Op::SyncSession {
                            session_id: app.current_session_id.clone(),
                            messages: app.api_messages.clone(),
                            system_prompt: app.system_prompt.clone(),
                            system_prompt_override: false,
                            model: app.model.clone(),
                            workspace: app.workspace.clone(),
                            mode: app.mode,
                        })
                        .await;
                }
            }
            AppAction::SendMessage(content) => {
                let queued = build_queued_message(app, content);
                dispatch_composer_message(
                    app,
                    config,
                    engine_handle,
                    queued,
                    DispatchRecovery::Immediate,
                    ComposerSubmitAction::Submit(app.decide_submit_disposition()),
                )
                .await?;
            }
            AppAction::WorkflowInstruction {
                display,
                instruction,
            } => {
                let queued = QueuedMessage::new(display, Some(instruction));
                dispatch_composer_message(
                    app,
                    config,
                    engine_handle,
                    queued,
                    DispatchRecovery::Immediate,
                    ComposerSubmitAction::Submit(app.decide_submit_disposition()),
                )
                .await?;
            }
            AppAction::SetGoalStatus { status, clear } => {
                accept_goal_control(
                    app,
                    engine_handle,
                    GoalControlIntent::SetStatus { status, clear },
                );
            }
            AppAction::SetGoalObjective {
                objective,
                token_budget,
            } => {
                accept_goal_control(
                    app,
                    engine_handle,
                    GoalControlIntent::SetObjective {
                        objective,
                        token_budget,
                    },
                );
            }
            AppAction::OpenTextPager { title, content } => {
                open_text_pager(app, title, content);
            }
            AppAction::VoiceCapture => {
                use commands::voice::VoiceCaptureOutcome;
                match commands::voice::capture_and_transcribe(app, config).await {
                    Ok(VoiceCaptureOutcome::Insert(text)) => {
                        app.insert_str(&text);
                        app.status_message = Some(format!(
                            "{}: {text}",
                            tr(app.ui_locale, MessageId::VoiceTranscribed)
                        ));
                    }
                    Ok(VoiceCaptureOutcome::Send(content)) => {
                        app.status_message =
                            Some(tr(app.ui_locale, MessageId::VoiceTranscribed).to_string());
                        let queued = build_queued_message(app, content);
                        dispatch_composer_message(
                            app,
                            config,
                            engine_handle,
                            queued,
                            DispatchRecovery::Immediate,
                            ComposerSubmitAction::Submit(app.decide_submit_disposition()),
                        )
                        .await?;
                    }
                    Err(err) => {
                        app.voice_enabled = false;
                        app.status_message = Some(err);
                    }
                }
            }
            AppAction::ListSubAgents => {
                // #3802: non-blocking send — refresh op, safe to drop.
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            AppAction::PreviewOutboundRequest {
                json,
                base_prompt_only,
                hypothetical_prompt,
            } => {
                // Split of authority: the host resolves the next turn's route
                // with the same planner it would use to send one, and the
                // engine — the only place that can rebuild the tool catalog,
                // MCP state, gates, system prompt, and prepared body — turns
                // that plan into a manifest.
                let inputs =
                    build_preview_request_inputs(app, config, engine_handle, hypothetical_prompt)
                        .await;
                if let Err(err) = engine_handle
                    .send(Op::PreviewOutboundRequest {
                        inputs: Box::new(inputs),
                        json,
                        base_prompt_only,
                    })
                    .await
                {
                    app.status_message = Some(format!("Cannot preview request: {err}"));
                }
            }
            AppAction::CancelSubAgent { agent_id } => {
                app.status_message = Some(format!("Cancelling {agent_id}..."));
                if engine_handle
                    .send(Op::CancelSubAgent {
                        agent_id: agent_id.clone(),
                    })
                    .await
                    .is_err()
                {
                    app.status_message = Some(format!("Could not cancel {agent_id}"));
                }
            }
            AppAction::FetchBalance => {
                let provider = app.api_provider;
                if !crate::config::provider_has_balance_api(provider) {
                    app.add_message(HistoryCell::System {
                        content: format!(
                            "Balance check is not supported for {} yet. Check the provider dashboard for account balance details.",
                            provider.display_name()
                        ),
                    });
                } else {
                    let api_key = config.deepseek_api_key().unwrap_or_default();
                    if api_key.trim().is_empty() {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "No API key configured for {}.",
                                provider.display_name()
                            ),
                        });
                    } else {
                        let base_url = config.deepseek_base_url();
                        match fetch_provider_balance(provider, &api_key, &base_url).await {
                            Some(info) => {
                                if let Ok(mut guard) = app.balance_cell.lock() {
                                    *guard = Some(info.clone());
                                }
                                app.last_balance_fetch = Some(Instant::now());
                                app.add_message(HistoryCell::System {
                                    content: info.report(provider.display_name()),
                                });
                            }
                            None => {
                                let fallback = app
                                    .balance_cell
                                    .lock()
                                    .ok()
                                    .and_then(|guard| guard.clone())
                                    .and_then(|info| {
                                        info.chip_label().map(|amount| {
                                            format!(
                                                "Could not refresh {} balance; last known: {amount}",
                                                provider.display_name()
                                            )
                                        })
                                    });
                                app.add_message(HistoryCell::System {
                                    content: fallback.unwrap_or_else(|| {
                                        format!(
                                            "Could not fetch {} account balance. Check the provider dashboard.",
                                            provider.display_name()
                                        )
                                    }),
                                });
                            }
                        }
                    }
                }
            }
            AppAction::FetchModels => {
                app.status_message = Some("Fetching models...".to_string());
                match fetch_available_models(config).await {
                    Ok(models) => {
                        app.add_message(HistoryCell::System {
                            content: format_helpers::available_models_message(
                                app.ui_locale,
                                app.provider_identity_for_persistence(),
                                &app.model,
                                &models,
                                &crate::fleet::members::fleet_models(&app.workspace),
                            ),
                        });
                        app.status_message = Some(format!("Found {} model(s)", models.len()));
                    }
                    Err(error) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Failed to fetch models from {}: {error}",
                                config.api_provider().display_name()
                            ),
                        });
                    }
                }
            }
            AppAction::RefreshModelsDevCatalog => {
                app.status_message = Some("Refreshing Models.dev catalog...".to_string());
                let message = match crate::models_dev_live::refresh(true).await {
                    Ok(count) => {
                        let status = crate::models_dev_live::status();
                        let source = if status.source_label.is_empty() {
                            "unknown"
                        } else {
                            status.source_label.as_str()
                        };
                        format!(
                            "Models.dev catalog refreshed: {count} offerings ({:?}, source {source})",
                            status.freshness
                        )
                    }
                    Err(err) => {
                        let status = crate::models_dev_live::status();
                        format!(
                            "Models.dev refresh failed ({err}); keeping prior/bundled rows ({} offerings, {:?})",
                            status.offering_count, status.freshness
                        )
                    }
                };
                app.add_message(HistoryCell::System {
                    content: message.clone(),
                });
                app.status_message = Some(message);
            }
            AppAction::CacheWarmup => {
                app.status_message = Some("Warming prompt cache...".to_string());
                match run_cache_warmup(app, config).await {
                    Ok(outcome) => {
                        app.session.last_base_url = Some(outcome.base_url.clone());
                        app.session.last_warmup_key = Some(CacheWarmupKey::from_inspection(
                            &outcome.provider_identity,
                            &outcome.model,
                            &outcome.base_url,
                            &outcome.inspection,
                        ));
                        let mut message = format_helpers::cache_warmup_result(&outcome.usage);
                        if let Some(key) = app.session.last_warmup_key.as_ref() {
                            message.push_str(&format!("\nWarmup key: {}", key.hash_short()));
                        }
                        // Append prefix-cache stability info.
                        if app.prefix_checks_total > 0 {
                            let changes = app.prefix_change_count;
                            let total = app.prefix_checks_total;
                            let stable = total.saturating_sub(changes);
                            let pct = app
                                .prefix_stability_pct
                                .map(|p| format!("{p}%"))
                                .unwrap_or_else(|| "--".to_string());
                            message.push_str(&format!(
                                "\n\nPrefix stability: {pct} ({stable}/{total} checks stable, {changes} change{})",
                                if changes == 1 { "" } else { "s" }
                            ));
                            if let Some(ref desc) = app.last_prefix_change_desc {
                                message.push_str(&format!("\nLast prefix change: {desc}"));
                            }
                        }
                        app.add_message(HistoryCell::System { content: message });
                        app.status_message = Some("Cache warmup complete".to_string());
                    }
                    Err(error) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Cache warmup failed: {error}"),
                        });
                        app.status_message = Some("Cache warmup failed".to_string());
                    }
                }
            }
            AppAction::SwitchProvider { provider, model } => {
                switch_provider(app, engine_handle, config, provider, model).await;
                let api_key = config.deepseek_api_key().unwrap_or_default();
                let base_url = config.deepseek_base_url();
                schedule_balance_fetch(app, &api_key, &base_url, false);
            }
            AppAction::SwitchModelRoute { provider, model } => {
                let previous_model = if app.auto_model {
                    "auto".to_string()
                } else {
                    app.model.clone()
                };
                // Hotbar route actions do not carry an effort choice. Preserve
                // the raw global preference instead of feeding a fixed
                // route's normalized live tier back through the picker path.
                let previous_effort = app
                    .reasoning_effort_preference
                    .unwrap_or(app.reasoning_effort);
                apply_model_picker_choice(
                    app,
                    engine_handle,
                    config,
                    model,
                    Some(provider),
                    None,
                    previous_effort,
                    previous_model,
                    previous_effort,
                    // A hotbar route switch is a session action, not a
                    // statement about what the next launch should open with.
                    false,
                )
                .await;
            }
            AppAction::UpdateCompaction(compaction) => {
                if app.is_loading || app.is_compacting {
                    let queued = try_apply_model_and_compaction_update(
                        engine_handle,
                        compaction,
                        app.mode,
                        app.active_route_limits,
                    );
                    app.status_message = Some(if queued {
                        "Config change queued; the active turn remains responsive.".to_string()
                    } else {
                        "Config change deferred; it will apply to the next turn.".to_string()
                    });
                } else {
                    apply_model_and_compaction_update(
                        engine_handle,
                        compaction,
                        app.mode,
                        app.active_route_limits,
                    )
                    .await;
                }
            }
            AppAction::UpdateStreamChunkTimeout(timeout_secs) => {
                let _ = engine_handle
                    .send(Op::SetStreamChunkTimeout { timeout_secs })
                    .await;
            }
            AppAction::UpdateSubagentRuntimeConfig {
                enabled,
                max_subagents,
                launch_concurrency,
                max_spawn_depth,
                api_timeout_secs,
                heartbeat_timeout_secs,
            } => {
                let _ = engine_handle
                    .send(Op::SetSubagentRuntimeConfig {
                        enabled,
                        max_subagents,
                        launch_concurrency,
                        max_spawn_depth,
                        api_timeout_secs,
                        heartbeat_timeout_secs,
                    })
                    .await;
            }
            AppAction::UpdateSearchProvider { provider } => {
                let effective_provider = config.set_search_provider(provider);
                let _ = engine_handle
                    .send(Op::SetSearchProvider {
                        provider: effective_provider,
                    })
                    .await;
            }
            AppAction::UpdatePromptSuggestion { enabled } => {
                config.prompt_suggestion = Some(enabled);
            }
            AppAction::UpdateNotification { update } => {
                config
                    .notifications
                    .get_or_insert_with(crate::config::NotificationsConfig::default)
                    .apply_update(update);
                let _ = crate::tui::notifications::settings(config);
            }
            AppAction::SetAdvisorEnabled { enabled } => {
                let _ = engine_handle.send(Op::SetAdvisorEnabled { enabled }).await;
            }
            AppAction::OpenConfigEditor(mode) => match mode {
                ConfigUiMode::Native => {
                    if app.view_stack.top_kind() != Some(ModalKind::Config) {
                        app.view_stack.push(ConfigView::new_for_app(app));
                    }
                }
                ConfigUiMode::Tui => {
                    pause_terminal(
                        terminal,
                        app.use_alt_screen(),
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                    )?;
                    let editor_result = config_ui::run_tui_editor(app, config)
                        .and_then(|doc| config_ui::apply_document(doc, app, config, true));
                    resume_terminal(
                        terminal,
                        app.use_alt_screen(),
                        app.use_mouse_capture,
                        app.use_bracketed_paste,
                        app.synchronized_output_enabled,
                    )?;
                    match editor_result {
                        Ok(outcome) => {
                            if outcome.requires_engine_sync {
                                apply_model_and_compaction_update(
                                    engine_handle,
                                    app.compaction_config(),
                                    app.mode,
                                    app.active_route_limits,
                                )
                                .await;
                            }
                            app.add_message(HistoryCell::System {
                                content: outcome.final_message.clone(),
                            });
                            app.status_message = Some(outcome.final_message);
                        }
                        Err(err) => {
                            app.add_message(HistoryCell::System {
                                content: format!("Config UI failed: {err}"),
                            });
                        }
                    }
                }
                ConfigUiMode::Web => {
                    #[cfg(feature = "web")]
                    {
                        let session = config_ui::start_web_editor(app, config).await?;
                        let url = format!("http://{}", session.addr);
                        let open_err = config_ui::open_browser(&url).err();
                        if let Some(err) = open_err {
                            app.add_message(HistoryCell::System {
                                content: format!("Failed to open browser automatically: {err}"),
                            });
                        }
                        app.status_message = Some(format!("web ui listen on: {url}"));
                        *web_config_session = Some(session);
                    }
                    #[cfg(not(feature = "web"))]
                    {
                        app.add_message(HistoryCell::System {
                            content: "This build does not include the web config UI.".to_string(),
                        });
                    }
                }
            },
            AppAction::OpenConfigView => {
                if app.view_stack.top_kind() != Some(ModalKind::Config) {
                    app.view_stack.push(ConfigView::new_for_app(app));
                }
            }
            AppAction::OpenWorktreeManager => {
                if app.view_stack.top_kind() != Some(ModalKind::WorktreeManager) {
                    // Non-blocking: git_status caches; manager never shells on paint.
                    crate::tui::git_status::refresh_if_stale(&app.workspace);
                    app.view_stack
                        .push(crate::tui::worktree_manager::WorktreeManagerView::new(
                            app.workspace.clone(),
                        ));
                }
            }
            AppAction::OpenModelPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ModelPicker) {
                    // Slash `/model` and the picker share one grammar: the
                    // composer must not keep a leftover `/model` buffer
                    // (or a held paste-burst) under the picker query.
                    app.clear_input();
                    app.paste_burst.clear_after_explicit_paste();
                    app.view_stack
                        .push(crate::tui::model_picker::ModelPickerView::new(app, config));
                }
            }
            AppAction::OpenProviderPicker => {
                open_provider_picker(app, config, engine_handle).await;
            }
            AppAction::OpenProviderSetup { provider } => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_setup(
                            app.api_provider,
                            provider,
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                    app.status_message = Some("Provider setup catalog opened.".to_string());
                }
            }
            AppAction::OpenDs4Setup => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_ds4_setup(
                            app.api_provider,
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                }
            }
            AppAction::OpenProviderTemplateList => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_template_list(
                            app.api_provider,
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                }
            }
            AppAction::OpenTemplateSetup { template_id } => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    if let Some(picker) =
                        crate::tui::provider_picker::ProviderPickerView::new_for_template_setup(
                            app.api_provider,
                            &template_id,
                            config,
                            runtime_status,
                        )
                    {
                        app.view_stack.push(
                            picker
                                .with_locale(app.ui_locale)
                                .with_provider_health(&app.provider_health),
                        );
                        let template = codewhale_config::provider_setup_template(&template_id);
                        let message = match template {
                            Some(template) if template.is_unpublished() => {
                                app.tr(MessageId::ProviderTemplateUnpublished).into_owned()
                            }
                            Some(template) if template.is_compatible() => app
                                .tr(MessageId::ProviderTemplateOpenedEnvOnly)
                                .replace("{id}", &template_id),
                            _ => app
                                .tr(MessageId::ProviderTemplateOpened)
                                .replace("{id}", &template_id),
                        };
                        let level = if template.is_some_and(|item| item.is_unpublished()) {
                            StatusToastLevel::Warning
                        } else {
                            StatusToastLevel::Info
                        };
                        app.push_status_toast(message, level, Some(8_000));
                    } else {
                        app.push_status_toast(
                            app.tr(MessageId::ProviderTemplateUnknown)
                                .replace("{id}", &template_id),
                            StatusToastLevel::Error,
                            Some(8_000),
                        );
                    }
                }
            }
            AppAction::StartXaiDeviceLogin => {
                let _switched =
                    run_xai_device_login_from_tui(terminal, app, engine_handle, config).await?;
            }
            AppAction::StartChatgptPkceLogin => {
                let _switched =
                    run_chatgpt_pkce_login_from_tui(terminal, app, engine_handle, config).await?;
            }
            AppAction::StartChatgptRevoke => {
                run_chatgpt_revoke_from_tui(app, config).await;
            }
            AppAction::SetScreenMode(mode) => {
                // The terminal transition is the only fallible part; a failed
                // probe leaves the previous screen live and says why.
                match switch_screen_mode(terminal, app, mode) {
                    Ok(()) => {
                        let screen = match mode {
                            crate::tui::app::ScreenMode::Fullscreen => {
                                app.tr(MessageId::ScreenModeFullscreenNotice)
                            }
                            crate::tui::app::ScreenMode::Inline => {
                                app.tr(MessageId::ScreenModeInlineNotice)
                            }
                        };
                        let capture = if app.use_mouse_capture {
                            app.tr(MessageId::ScreenModeMouseCaptureOn)
                        } else {
                            app.tr(MessageId::ScreenModeMouseCaptureOff)
                        };
                        app.add_message(HistoryCell::System {
                            content: format!("{screen} {capture}"),
                        });
                    }
                    Err(reason) => {
                        let unchanged = app
                            .tr(MessageId::ScreenModeUnchanged)
                            .replace("{reason}", &reason);
                        app.add_message(HistoryCell::System {
                            content: unchanged.clone(),
                        });
                        app.push_status_toast(
                            unchanged.trim_end_matches('.').to_string(),
                            StatusToastLevel::Warning,
                            Some(8_000),
                        );
                    }
                }
            }
            AppAction::OpenModePicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ModePicker) {
                    app.view_stack
                        .push(crate::tui::views::mode_picker::ModePickerView::new(
                            app.mode,
                            app.ui_locale,
                        ));
                }
            }
            AppAction::OpenStatusPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::StatusPicker) {
                    app.view_stack
                        .push(crate::tui::views::status_picker::StatusPickerView::new(
                            &app.status_items,
                            app.api_provider,
                            app.ui_locale,
                        ));
                }
            }
            AppAction::OpenFeedbackPicker => {
                if app.view_stack.top_kind() != Some(ModalKind::FeedbackPicker) {
                    app.view_stack
                        .push(crate::tui::feedback_picker::FeedbackPickerView::new());
                }
            }
            AppAction::OpenThemePicker => {
                if app.view_stack.top_kind() != Some(ModalKind::ThemePicker) {
                    // Capture the active theme name straight from `app` so
                    // Esc can revert through the same ConfigUpdated channel.
                    // Avoids re-reading settings.toml from disk on every
                    // `/theme` invocation.
                    let original = app.theme_id.name().to_string();
                    app.view_stack
                        .push_boxed(crate::tui::theme_picker::ThemePickerView::boxed(
                            original,
                            app.ui_locale,
                            app.background_color_override,
                        ));
                }
            }
            AppAction::OpenSkillsManager => {
                if app.view_stack.top_kind() != Some(ModalKind::SkillsManager) {
                    app.view_stack
                        .push(crate::tui::views::skills_manager::SkillsManagerView::new(
                            app,
                        ));
                }
            }
            AppAction::OpenWorkflowsManager => {
                if app.view_stack.top_kind() != Some(ModalKind::WorkflowsManager) {
                    app.view_stack
                        .push(crate::tui::views::workflows_manager::WorkflowsManagerView::new(app));
                }
            }
            AppAction::OpenExtensions { tab } => {
                if app.view_stack.top_kind() != Some(ModalKind::Extensions) {
                    app.view_stack
                        .push(crate::tui::views::extensions::ExtensionsView::new(app, tab));
                }
            }
            AppAction::OpenFleetList => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetList) {
                    app.view_stack
                        .push(crate::tui::views::fleet_list::FleetListView::new(
                            app, config,
                        ));
                }
            }
            AppAction::OpenFleetRoster => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetRoster) {
                    app.view_stack
                        .push(crate::tui::views::fleet_roster::FleetRosterView::new(
                            app, config,
                        ));
                }
            }
            AppAction::OpenFleetSetup => {
                open_fleet_setup_target(app, config, None);
            }
            AppAction::FleetAddModel {
                provider,
                model,
                roles,
            } => {
                use crate::commands::{fleet_catalog_rejection, fleet_provider_rejection};
                let locale = app.ui_locale;
                // The live `config` is the provider truth: the startup
                // snapshot went stale after any in-session provider change.
                let rejection = fleet_provider_rejection(app, config, &provider)
                    .or_else(|| fleet_catalog_rejection(locale, &provider, &model));
                let content = match rejection {
                    Some(rejection) => rejection,
                    None => match crate::fleet::members::add_fleet_model(
                        &app.workspace,
                        &provider,
                        &model,
                        &roles,
                    ) {
                        Ok(change) => {
                            if !matches!(
                                change,
                                crate::fleet::members::FleetModelChange::Unchanged { .. }
                            ) {
                                app.fleet_roster_stale = true;
                            }
                            crate::fleet::members::change_receipt(
                                locale, &provider, &model, &change,
                            )
                        }
                        Err(error) => tr(locale, MessageId::FleetAddFailed)
                            .replace("{error}", &error.message(locale)),
                    },
                };
                app.add_message(HistoryCell::System { content });
            }
            AppAction::FleetRemoveModel { provider, model } => {
                let locale = app.ui_locale;
                let content = match crate::fleet::members::remove_fleet_model(
                    &app.workspace,
                    &provider,
                    &model,
                ) {
                    Ok(change) => {
                        app.fleet_roster_stale = true;
                        crate::fleet::members::change_receipt(locale, &provider, &model, &change)
                    }
                    Err(error) => tr(locale, MessageId::FleetRemoveFailed)
                        .replace("{error}", &error.message(locale)),
                };
                app.add_message(HistoryCell::System { content });
            }
            AppAction::OpenHotbarSetup => {
                if app.view_stack.top_kind() != Some(ModalKind::HotbarSetup) {
                    app.view_stack
                        .push(crate::tui::hotbar::setup::HotbarSetupView::new(app, config));
                }
            }
            AppAction::OpenSetupWizard => {
                if app.view_stack.top_kind() != Some(ModalKind::SetupWizard) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::setup::SetupWizardView::new_for_app(app, config));
                }
            }
            AppAction::OpenSetupWizardAt { step } => {
                if app.view_stack.top_kind() != Some(ModalKind::SetupWizard) {
                    let _ = app.next_draft_gen();
                    app.view_stack
                        .push(crate::tui::setup::SetupWizardView::new_for_app_at(
                            app, config, step,
                        ));
                }
            }
            AppAction::UseBundledConstitution => use_bundled_constitution(app, config),
            AppAction::PreviewEffectiveBasePrompt => preview_effective_base_prompt(app, config),
            AppAction::DisableHotbar => disable_hotbar(app, config),
            AppAction::RestoreHotbarDefaults => restore_hotbar_defaults(app, config),
            AppAction::OpenExternalUrl { url, label } => match open_external_url(&url) {
                Ok(()) => {
                    app.status_message = Some(format!("Opened {label} in your browser"));
                }
                Err(err) => {
                    app.add_message(HistoryCell::System {
                        content: format!(
                            "Could not open {label} automatically: {err}\n\nThe URL is printed above."
                        ),
                    });
                }
            },
            AppAction::OpenContextInspector => {
                open_context_inspector(app);
            }
            AppAction::OpenLiveTranscript => {
                open_live_transcript_overlay(app);
            }
            AppAction::OpenTurnInspector => {
                open_turn_inspector_pager(app);
            }
            AppAction::CompactContext { focus } => {
                try_queue_manual_compaction(app, config, engine_handle, focus);
            }
            AppAction::PurgeContext => {
                app.status_message = Some("Agent purging context...".to_string());
                let _ = engine_handle.send(Op::PurgeContext).await;
            }
            AppAction::TaskAdd { prompt } => {
                let owner_session_id = app
                    .current_session_id
                    .clone()
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                app.current_session_id = Some(owner_session_id.clone());
                let request = NewTaskRequest {
                    prompt: prompt.clone(),
                    model: Some(app.model.clone()),
                    workspace: Some(app.workspace.clone()),
                    mode: Some(task_mode_label(app.mode).to_string()),
                    allow_shell: Some(app.allow_shell),
                    trust_mode: Some(app.trust_mode),
                    auto_approve: Some(app_auto_approve_enabled(app)),
                    owner_session_id: Some(owner_session_id),
                };
                match task_manager.add_task(request).await {
                    Ok(task) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Task queued: {} ({})",
                                task.id,
                                summarize_tool_output(&task.prompt)
                            ),
                        });
                        app.status_message = Some(format!("Queued {}", task.id));
                    }
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Failed to queue task: {err}"),
                        });
                    }
                }
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::TaskList => {
                let tasks = match app.current_session_id.as_deref() {
                    Some(session_id) => {
                        task_manager
                            .list_tasks_for_owner(Some(30), None, session_id)
                            .await
                    }
                    None => Vec::new(),
                };
                refresh_active_task_panel(app, task_manager).await;
                app.add_message(HistoryCell::System {
                    content: format_task_list(&tasks),
                });
            }
            AppAction::RemoteControl(action) => match action {
                crate::remote_control::RemoteControlAction::Start => {
                    start_remote_control_session(app, config);
                }
                crate::remote_control::RemoteControlAction::Stop => {
                    app.remote_control.stop();
                    let status = app.remote_control.status_line();
                    app.sticky_status = None;
                    app.status_message = Some(status);
                }
            },
            AppAction::TaskShow { id } => {
                let task = match app.current_session_id.as_deref() {
                    Some(session_id) => task_manager.get_task_for_owner(&id, session_id).await,
                    None => Err(anyhow::anyhow!("Task not found: {id}")),
                };
                match task {
                    Ok(task) => open_task_pager(app, &task),
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Task lookup failed: {err}"),
                        });
                    }
                }
            }
            AppAction::TaskCancel { id } => {
                let cancellation = match app.current_session_id.as_deref() {
                    Some(session_id) => task_manager.cancel_task_for_owner(&id, session_id).await,
                    None => Err(anyhow::anyhow!("Task not found: {id}")),
                };
                match cancellation {
                    Ok(cancellation) => {
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Task {} status: {:?}",
                                cancellation.task.id, cancellation.task.status
                            ),
                        });
                    }
                    Err(err) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Task cancel failed: {err}"),
                        });
                    }
                }
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::Automation(action) => {
                crate::tui::automation_routing::handle_action(app, action, task_manager).await;
            }
            AppAction::ShellJob(action) => {
                handle_shell_job_action(app, action);
                // Immediately sync the task panel after cancel/poll so the
                // Activity sidebar stays accurate without waiting for the
                // next 2.5 s periodic refresh (#2937).
                refresh_active_task_panel(app, task_manager).await;
            }
            AppAction::Mcp(action) => {
                handle_mcp_ui_action(app, engine_handle, config, action).await;
            }
            AppAction::SwitchWorkspace { workspace } => {
                switch_workspace(app, engine_handle, task_manager, config, workspace).await;
            }
            AppAction::SwitchProfile { profile } => {
                let previous_profile = app.config_profile.clone();
                match Config::load(app.config_path.clone(), Some(&profile)).and_then(|new_config| {
                    validated_profile_default_route(&new_config)
                        .map(|validated_route| (new_config, validated_route))
                }) {
                    Ok((new_config, validated_route)) => {
                        let new_model = validated_route.model.clone();
                        let provider_identity = validated_route.identity.clone();
                        let route_limits = crate::route_budget::known_route_limits(
                            validated_route.candidate.limits(),
                        );
                        app.config_profile = Some(profile.clone());
                        *config = new_config.clone();
                        app.set_provider_identity_record(provider_identity);
                        app.billing_presentation =
                            crate::route_billing::for_route(config, app.api_provider);
                        app.set_model_selection(new_model.clone());
                        app.set_active_context_window_override(
                            config.context_window_for_provider_config(app.api_provider),
                        );
                        app.active_route_limits = route_limits;
                        app.update_model_compaction_budget();
                        app.session.last_prompt_tokens = None;
                        app.session.last_completion_tokens = None;
                        app.session.last_output_throughput = None;
                        // Rebuild the engine with the new config so API key/model/base URL take effect.
                        let _ = engine_handle.send(Op::Shutdown).await;
                        let engine_config = build_engine_config(app, config);
                        *engine_handle = spawn_tui_engine(engine_config, config);
                        if !app.api_messages.is_empty() {
                            let _ = engine_handle
                                .send(Op::SyncSession {
                                    session_id: app.current_session_id.clone(),
                                    messages: app.api_messages.clone(),
                                    system_prompt: app.system_prompt.clone(),
                                    system_prompt_override: false,
                                    model: app.model.clone(),
                                    workspace: app.workspace.clone(),
                                    mode: app.mode,
                                })
                                .await;
                        }
                        app.add_message(HistoryCell::System {
                            content: format!(
                                "Switched to profile '{profile}'. Model: {new_model}, Provider: {}",
                                app.provider_identity_for_persistence()
                            ),
                        });
                        app.status_message = Some(format!("Profile: {profile}"));
                    }
                    Err(err) => {
                        app.config_profile = previous_profile;
                        app.status_message =
                            Some(format!("Failed to switch to profile '{profile}': {err}"));
                    }
                }
            }
            AppAction::ShareSession {
                history_len: _,
                model,
                mode,
            } => {
                let status = if app.api_messages.is_empty() {
                    "No session content to share.".to_string()
                } else {
                    let history_json = serde_json::to_string_pretty(&app.api_messages)
                        .unwrap_or_else(|_| "[]".to_string());
                    match crate::commands::share::perform_share(&history_json, &model, &mode).await
                    {
                        Ok(url) => format!("Session shared! URL: {url}"),
                        Err(err) => format!("Share failed: {err}"),
                    }
                };
                app.add_message(HistoryCell::System {
                    content: status.clone(),
                });
                app.status_message = Some(status);
            }
        }
    }

    Ok(false)
}

pub(crate) fn apply_workspace_runtime_state(app: &mut App, config: &Config, workspace: PathBuf) {
    app.workspace = workspace.clone();
    app.coordination_detail = None;
    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&workspace);
    for error in crate::commands::user_registry::install_plugin_registry(
        &workspace,
        app.plugin_registry.as_ref(),
    ) {
        tracing::warn!(target: "plugins", "{error}");
    }
    // A plugin that failed to load used to be invisible until someone
    // happened to open /plugin. Surface a one-line hint instead of leaving
    // the discovery result buried in the trace log; warnings stay quiet.
    let plugin_load_errors = app
        .plugin_registry
        .diagnostics()
        .iter()
        .filter(|diagnostic| {
            diagnostic.level == crate::plugins::types::PluginDiagnosticLevel::Error
        })
        .count();
    if plugin_load_errors > 0 {
        app.status_message = Some(if plugin_load_errors == 1 {
            "1 plugin failed to load — /plugin for details".to_string()
        } else {
            format!("{plugin_load_errors} plugins failed to load — /plugin for details")
        });
    }
    app.active_skill = None;
    app.active_skill_provenance = None;
    // Switching workspace reloads the hook set (project hooks are per-repo)
    // but stays inside the same TUI session, so the session id is preserved.
    app.hooks = app.hooks.rebind(
        crate::hooks::HooksConfig::load_with_project_and_plugins(
            config.hooks_config(),
            &workspace,
            Some(app.plugin_registry.as_ref()),
        ),
        workspace.clone(),
    );
    app.skills_dir = crate::tui::app::resolve_skills_dir(&workspace, &config.skills_dir(), config);
    app.skills_scan_codewhale_only = config.skills_config().scan_codewhale_only();
    app.project_context_pack_enabled = config.project_context_pack_enabled();
    app.refresh_skill_cache();
    app.workspace_context = None;
    if let Ok(mut cell) = app.workspace_context_cell.lock() {
        *cell = None;
    }
    app.workspace_context_refreshed_at = None;
    app.file_tree = None;

    let shell_manager = crate::tools::shell::new_shared_shell_manager(workspace);
    app.runtime_services.shell_manager = Some(shell_manager);
    app.runtime_services.hook_executor = Some(std::sync::Arc::new(app.hooks.clone()));
}

pub(crate) fn apply_hotbar_setup_saved(
    app: &mut App,
    config: &mut Config,
    bindings: Vec<codewhale_config::HotbarBindingToml>,
) {
    match crate::config_persistence::persist_hotbar_bindings(app.config_path.as_deref(), &bindings)
    {
        Ok(path) => {
            config.hotbar = Some(bindings);
            app.status_message = Some(format!("Hotbar bindings saved to {}", path.display()));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to save Hotbar bindings: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to save Hotbar bindings: {err}"),
            });
        }
    }
    app.needs_redraw = true;
}

pub(crate) async fn apply_approval_decision(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    event: ApprovalDecisionEvent,
) {
    if event.decision == ReviewDecision::ApprovedForSession {
        // Only the lossy grouping key is stored: a session grant is scoped to
        // the command family (e.g. `shell:git status`), never to the whole
        // tool — approving one shell command must not approve every shell
        // command for the session (ops R2). The tool name is recorded as
        // audit evidence, not as a grant.
        crate::audit::log_sensitive_event(
            "tool.approval.session_grant",
            serde_json::json!({
                "tool_name": event.tool_name,
                "grouping_key": event.approval_grouping_key,
            }),
        );
        app.approval_session_approved
            .insert(event.approval_grouping_key.clone());
    }

    if matches!(
        event.decision,
        ReviewDecision::Approved | ReviewDecision::ApprovedForSession
    ) && !event.persistent_rules.is_empty()
        && !event.timed_out
    {
        persist_rules_from_approval(app, config, &event.persistent_rules);
    }

    match event.decision {
        ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {
            // Mirror mode: clear the shared-approval gate so a late web
            // decision acks "no longer pending" instead of double-answering.
            app.remote_control
                .resolve_pending_approval(&event.tool_id, true);
            let _ = engine_handle.approve_tool_call(event.tool_id).await;
        }
        ReviewDecision::Denied => {
            // Cache the denial so the model retry-loop doesn't re-prompt for
            // the exact same approval_key (#360). Only the key (per-call
            // unique) is stored — NOT the tool_name, which would block all
            // future invocations of the same tool type (#1377).
            if !event.timed_out {
                app.approval_session_denied.insert(event.approval_key);
            }
            app.remote_control
                .resolve_pending_approval(&event.tool_id, false);
            let _ = engine_handle.deny_tool_call(event.tool_id).await;
        }
        ReviewDecision::Abort => {
            engine_handle.cancel();
            mark_active_turn_cancelled_locally(app);
            app.status_message = Some(parent_stop_status(app, "Request cancelled"));
        }
    }
}

pub(crate) fn apply_setup_runtime_preset(
    app: &mut App,
    config: &mut Config,
    preset: crate::tui::setup::SetupRuntimePreset,
    state: codewhale_config::SetupState,
) -> Result<String> {
    if let Some(source) = config.runtime_preset_blocker(
        app.config_path.as_deref(),
        app.config_profile.as_deref(),
        &app.workspace,
    ) {
        anyhow::bail!(
            "Runtime presets cannot override {source}; change that controlling source first"
        );
    }
    if preset == crate::tui::setup::SetupRuntimePreset::HighTrustLocal {
        let approval = config.approval_policy_control(
            app.config_path.as_deref(),
            app.config_profile.as_deref(),
            &app.workspace,
        );
        if !approval.editable_root() {
            anyhow::bail!(
                "Full Access cannot override {}; change that controlling source first",
                approval.label()
            );
        }
    }

    let settings_path = Settings::path().context("failed to resolve settings path")?;
    let settings_snapshot = RuntimePresetFileSnapshot::capture(settings_path)?;
    // The preset's settings read, its config-document write, and its settings
    // write are one durable transaction with file-snapshot rollback. Hold the
    // settings transaction lock across all of it so a concurrent writer (a queued
    // mode/thinking drain, the Shift+Tab posture write) can neither be lost by
    // this save nor be reverted by the rollback.
    // Every durable write happens inside this closure, so the settings lock is
    // released before live state moves below.
    crate::settings::with_settings_transaction(|settings_transaction| {
        let mut settings = settings_transaction
            .load()
            .context("failed to load settings")?;
        settings.default_mode = preset.default_mode().to_string();
        settings.permission_posture = Some(preset.permission_posture().to_string());

        // Persist into the same file Config::load actually selected. A missing
        // explicit env target remains authoritative for both reads and writes;
        // an invalid target fails here instead of selecting a different file.
        let selected_config_path =
            crate::config::resolve_load_config_path(app.config_path.clone())?
                .or_else(|| app.config_path.clone());
        let config_path =
            crate::config_persistence::config_toml_path(selected_config_path.as_deref())
                .context("failed to resolve config path")?;
        let config_snapshot = RuntimePresetFileSnapshot::capture(config_path.clone())?;
        if let Err(error) =
            crate::config_persistence::mutate_config_document(&config_path, |document| {
                if let Some(policy) = preset.approval_policy() {
                    crate::config_persistence::set_document_value(
                        document,
                        &["approval_policy"],
                        policy,
                    )?;
                } else {
                    crate::config_persistence::unset_document_value(
                        document,
                        &["approval_policy"],
                    )?;
                }
                crate::config_persistence::set_document_value(
                    document,
                    &["allow_shell"],
                    preset.allow_shell(),
                )?;
                crate::config_persistence::set_document_value(
                    document,
                    &["sandbox_mode"],
                    preset.sandbox_mode(),
                )
            })
            .context("failed to persist runtime posture")
        {
            return Err(runtime_preset_error_with_rollback(
                error,
                &[&settings_snapshot, &config_snapshot],
            ));
        }
        if let Err(error) = settings_transaction
            .save(&settings)
            .context("failed to save settings")
        {
            return Err(runtime_preset_error_with_rollback(
                error,
                &[&settings_snapshot, &config_snapshot],
            ));
        }
        if let Err(error) = state
            .save()
            .context("failed to persist setup runtime posture state")
        {
            return Err(runtime_preset_error_with_rollback(
                error,
                &[&settings_snapshot, &config_snapshot],
            ));
        }
        Ok(())
    })?;

    // Durable writes succeeded as one transaction. Only now may live state
    // move to the new posture.
    if let Some(policy) = preset.approval_policy() {
        config.approval_policy = Some(policy.to_string());
        app.mark_approval_policy_locked();
    } else {
        config.approval_policy = None;
        app.clear_saved_approval_policy_lock();
    }
    config.allow_shell = Some(preset.allow_shell());
    config.sandbox_mode = Some(preset.sandbox_mode().to_string());
    app.configured_sandbox_mode = config.sandbox_mode.clone();
    app.configured_sandbox_network = config.sandbox_network_access;

    let approval_mode = ApprovalMode::from_config_value(
        preset
            .approval_policy()
            .unwrap_or(preset.permission_posture()),
    )
    .unwrap_or(ApprovalMode::Suggest);
    let trust_mode = match preset {
        crate::tui::setup::SetupRuntimePreset::AskFirst => false,
        crate::tui::setup::SetupRuntimePreset::NormalAgent => app.agent_trust_baseline(),
        crate::tui::setup::SetupRuntimePreset::HighTrustLocal => true,
    };
    app.set_agent_runtime_baseline(preset.allow_shell(), trust_mode, approval_mode);
    let mode = AppMode::from_setting(preset.default_mode());
    app.set_mode(mode);
    app.needs_redraw = true;

    Ok(format!("Applied {}.", preset.result_summary()))
}

pub(crate) fn apply_backtrack(app: &mut App, depth: usize) {
    let Some(history_idx) = find_user_cell_index_from_tail(app, depth) else {
        app.status_message = Some("Backtrack target no longer present".to_string());
        return;
    };

    // Snapshot the user text before truncating so we can refill the
    // composer.
    let user_text = match app.history.get(history_idx) {
        Some(HistoryCell::User { content }) => content.clone(),
        _ => String::new(),
    };

    // Trim the visible transcript at the chosen user cell. Per-cell
    // revisions and tool-cell maps are kept consistent through
    // `App::truncate_history_to`.
    app.truncate_history_to(history_idx);

    // Trim the API-message log at the matching user PROMPT. `depth` counts
    // visible `HistoryCell::User` cells (real prompts), but a naive
    // `role == "user"` walk over `api_messages` over-counts: tool results are
    // stored as `role == "user"` messages too, so in any turn with tool calls
    // the cut would land mid-turn on a tool_result — leaving a dangling
    // assistant tool_use with no matching result and a transcript the provider
    // rejects. Count only messages that actually yield a User cell, the same
    // predicate `apply_loaded_session` uses.
    if let Some(idx) = backtrack_api_cut_index(&app.api_messages, depth) {
        app.api_messages.truncate(idx);
    }

    // Hand the dropped text back to the user so they can edit + resend.
    app.input = user_text;
    app.cursor_position = app.input.chars().count();

    // Close the overlay, refresh sticky-tail flag, and surface a hint.
    if app.view_stack.top_kind() == Some(ModalKind::LiveTranscript) {
        app.view_stack.pop();
    }
    app.status_message =
        Some("Rewound to previous user message — edit and Enter to resend".to_string());
    app.scroll_to_bottom();
    app.mark_history_updated();
    app.needs_redraw = true;
}

pub(crate) async fn apply_provider_picker_custom_provider(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    provider_id: String,
    base_url: String,
    model: Option<String>,
    api_key_env: Option<String>,
) -> bool {
    let written = match crate::config_persistence::persist_custom_provider(
        app.config_path.as_deref(),
        &provider_id,
        &base_url,
        model.as_deref(),
        api_key_env.as_deref(),
    ) {
        Ok(path) => path,
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!("Failed to save custom provider {provider_id}: {err}"),
            });
            app.status_message = Some("Custom provider was not saved.".to_string());
            return false;
        }
    };

    config.provider = Some(provider_id.clone());
    let entry = config
        .providers
        .get_or_insert_with(ProvidersConfig::default)
        .custom
        .entry(provider_id.clone())
        .or_default();
    entry.kind = Some("openai-compatible".to_string());
    entry.base_url = Some(base_url.trim().trim_end_matches('/').to_string());
    if provider_id == "ds4" && crate::config::base_url_uses_local_host(&base_url) {
        entry.context_window = Some(100_000);
    }
    entry.model = model.clone().and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    });
    let keyless_local = provider_id == "ds4"
        && api_key_env
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        && crate::config::base_url_uses_local_host(&base_url);
    entry.api_key_env = api_key_env.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    });
    entry.auth_mode = keyless_local.then(|| "none".to_string());

    app.status_message = Some(format!(
        "Custom provider {provider_id} saved to {}",
        written.display()
    ));
    switch_provider(app, engine_handle, config, ApiProvider::Custom, model).await
}

async fn reopen_provider_picker_list(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &Config,
    selected_provider_id: Option<String>,
    catalog_view: bool,
) {
    let runtime_status = query_provider_runtime_status(engine_handle).await;
    app.provider_picker_memory = Some(crate::tui::app::ProviderPickerMemory {
        catalog_view,
        selected_provider_id,
    });
    app.view_stack.push(
        crate::tui::provider_picker::ProviderPickerView::new_with_runtime_status_and_memory(
            app.api_provider,
            config,
            runtime_status,
            app.provider_picker_memory.as_ref(),
        )
        .with_locale(app.ui_locale)
        .with_provider_health(&app.provider_health),
    );
    app.needs_redraw = true;
}

pub(crate) async fn apply_provider_picker_test_connection(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    catalog_view: bool,
) {
    apply_provider_picker_test_connection_with_verifier(
        app,
        engine_handle,
        config,
        identity,
        catalog_view,
        &LiveProviderKeyVerifier,
    )
    .await;
}

fn sanitize_probe_status(reason: &str, api_key: &str) -> String {
    let mut text = reason.to_string();
    if let Some(rest) = reason.strip_prefix("HTTP ")
        && let Some((code, body)) = rest.split_once(':')
        && let Ok(status) = code.trim().parse::<u16>()
    {
        text = crate::llm_client::sanitize_http_error_body(None, status, body.trim());
    }
    let secret = api_key.trim();
    if !secret.is_empty() {
        text = text.replace(secret, "***");
    }
    crate::utils::truncate_with_ellipsis(text.trim(), 120, "…")
}

pub(crate) async fn apply_provider_picker_test_connection_with_verifier(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    catalog_view: bool,
    verifier: &dyn ProviderKeyVerifier,
) {
    let provider = identity.provider;
    let mut scoped_config = config.clone();
    scoped_config.provider = Some(identity.key.clone());
    let selected_id = if provider == ApiProvider::Custom {
        Some(identity.key.clone())
    } else {
        Some(provider.as_str().to_string())
    };
    if !crate::client::provider_api_key_verification_is_observed(provider) {
        app.push_status_toast(
            app.tr(MessageId::ProviderTestConnectionNoEndpoint)
                .replace("{provider}", &identity.key),
            StatusToastLevel::Warning,
            Some(8_000),
        );
        reopen_provider_picker_list(app, engine_handle, config, selected_id, catalog_view).await;
        return;
    }
    let api_key = match scoped_config.deepseek_api_key_read_only() {
        Ok(key) if !key.trim().is_empty() => key,
        _ => {
            app.push_status_toast(
                app.tr(MessageId::ProviderTestConnectionNeedKey)
                    .replace("{provider}", &identity.key),
                StatusToastLevel::Warning,
                Some(8_000),
            );
            reopen_provider_picker_list(app, engine_handle, config, selected_id, catalog_view)
                .await;
            return;
        }
    };
    let base_url = scoped_config.deepseek_base_url();
    let model = scoped_config.default_model();
    match verifier.verify(provider, &api_key, &base_url).await {
        Ok(()) => {
            app.provider_health
                .record_models_probe_success(&scoped_config, provider, &model);
            app.push_status_toast(
                app.tr(MessageId::ProviderConnectionChecked).into_owned(),
                StatusToastLevel::Success,
                Some(8_000),
            );
        }
        Err(reason) => {
            let safe = sanitize_probe_status(&reason, &api_key);
            app.provider_health.record_models_probe_failure(
                &scoped_config,
                provider,
                &model,
                provider_verification_error_category(&reason),
                &safe,
            );
            app.push_status_toast(
                app.tr(MessageId::ProviderTestConnectionFailed)
                    .replace("{provider}", &identity.key)
                    .replace("{error}", &safe),
                StatusToastLevel::Error,
                Some(8_000),
            );
        }
    }
    reopen_provider_picker_list(app, engine_handle, config, selected_id, catalog_view).await;
}

pub(crate) async fn apply_provider_picker_api_key(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
    base_url: Option<String>,
) {
    apply_provider_picker_api_key_with_verifier(
        app,
        engine_handle,
        config,
        identity,
        api_key,
        base_url,
        &LiveProviderKeyVerifier,
    )
    .await;
}

pub(crate) async fn apply_provider_picker_api_key_with_verifier(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
    base_url_override: Option<String>,
    verifier: &dyn ProviderKeyVerifier,
) {
    let provider = identity.provider;
    let mut scoped_config = config.clone();
    scoped_config.provider = Some(identity.key.clone());
    // #4526: a billing route chosen in the wizard is applied to the scoped
    // clone only, so the key is probed against the endpoint it will be saved
    // for without touching the on-disk config before the user confirms.
    if let Some(base_url) = base_url_override.clone() {
        scoped_config.set_provider_base_url_override(provider, Some(base_url));
    }
    // #3875: verify the key against the provider before opening the rest of
    // the guided flow. Nothing is persisted until the confirm stage.
    // Resolve the effective route, including compatibility routes whose
    // endpoint is selected by auth mode (notably a legacy Kimi CLI import).
    // This prevents a replacement Kimi Code API key from being probed against
    // the ordinary Moonshot endpoint.
    let base_url = scoped_config.deepseek_base_url();
    match verifier.verify(provider, &api_key, &base_url).await {
        Ok(()) => {
            // Keep the readiness row aligned with the live check the wizard
            // just completed. This probe only proves the endpoint and
            // credentials are reachable: the model is chosen after the probe,
            // so record a distinct connection-checked state rather than
            // claiming the model is ready. Providers without a real `/models`
            // probe remain unchecked.
            if crate::client::provider_api_key_verification_is_observed(provider) {
                let verified_model = scoped_config.default_model();
                app.provider_health.record_models_probe_success(
                    &scoped_config,
                    provider,
                    &verified_model,
                );
            }
            // Key is valid — continue the guided flow at model pick without
            // writing the secret yet.
            let runtime_status = query_provider_runtime_status(engine_handle).await;
            if let Some(picker) =
                crate::tui::provider_picker::ProviderPickerView::new_for_model_pick_after_validation(
                    app.api_provider,
                    provider,
                    &scoped_config,
                    runtime_status,
                    api_key,
                    base_url_override,
                )
                .map(|picker| {
                    picker
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health)
                })
            {
                app.view_stack.push(picker);
                app.status_message = Some(
                    app.tr(MessageId::ProviderConnectionCheckedPickModel)
                        .into_owned(),
                );
            } else {
                app.status_message = Some(format!(
                    "{} connection checked (/models returned 2xx), but the guided setup could not be re-opened.",
                    provider.as_str()
                ));
            }
            app.needs_redraw = true;
        }
        Err(reason) => {
            // Verification failed - keep the picker open at the key-entry
            // stage with the provider's actual error so the user can fix
            // the key instead of dead-ending with a status toast.
            let runtime_status = query_provider_runtime_status(engine_handle).await;
            if let Some(picker) =
                crate::tui::provider_picker::ProviderPickerView::new_for_key_entry_with_error(
                    app.api_provider,
                    provider,
                    &scoped_config,
                    runtime_status,
                    reason,
                )
                .map(|picker| {
                    picker
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health)
                })
            {
                app.view_stack.push(picker);
                app.status_message = Some(format!(
                    "{} API key verification failed - check the key and try again.",
                    provider.as_str()
                ));
            } else {
                app.status_message = Some(format!(
                    "{} API key verification failed, but the provider could not be re-opened.",
                    provider.as_str()
                ));
            }
            app.needs_redraw = true;
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn apply_provider_picker_setup_confirmed(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    identity: crate::config::ProviderIdentity,
    api_key: String,
    model: String,
    context_window: Option<u32>,
    base_url: Option<String>,
) -> bool {
    use crate::config::{
        save_api_key_for_identity, save_provider_base_url_for_identity,
        save_provider_context_window_for_identity, save_provider_model_for_identity,
    };

    let provider = identity.provider;

    let model = model.trim().to_string();
    if model.is_empty() {
        app.add_message(HistoryCell::System {
            content: format!(
                "Cannot finish {} setup: default model is empty.\nProvider unchanged.",
                provider.as_str()
            ),
        });
        return false;
    }

    // #4526: the wizard's billing-route choice is written before the key so the
    // credential is saved onto the route it was verified against. It lands only
    // in that provider's own `base_url`; failing here aborts before any secret
    // is persisted rather than leaving a key on the wrong endpoint.
    if let Some(base_url) = base_url.as_deref() {
        if let Err(err) = save_provider_base_url_for_identity(&identity, config, base_url) {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to save {} endpoint `{base_url}`: {err}\nProvider unchanged.",
                    provider.as_str()
                ),
            });
            return false;
        }
        config.set_provider_base_url_override(provider, Some(base_url.to_string()));
    }

    // Persist key first via the existing comment-preserving path, then pin the
    // chosen default model on the same document when the provider uses a
    // `[providers.<name>]` table.
    let mut save_confirmation = None;
    match save_api_key_for_identity(&identity, config, &api_key) {
        Ok(saved) => {
            // #5195: name where the key actually landed (secret store backend
            // + credential-free config metadata) and the scope it is visible
            // from — credential writes are rescoped to the user-global config,
            // so the key is available in every folder.
            let destination = saved.describe();
            if let Err(err) = save_provider_model_for_identity(&identity, config, &model) {
                app.add_message(HistoryCell::System {
                    content: format!(
                        "Saved {} API key to {destination} (available in all folders), but failed to pin model `{model}`: {err}",
                        provider.as_str(),
                    ),
                });
            } else if let Some(context_window) = context_window {
                if let Err(err) =
                    save_provider_context_window_for_identity(&identity, config, context_window)
                {
                    app.add_message(HistoryCell::System {
                        content: format!(
                            "Saved {} API key and model to {destination} (available in all folders), but failed to save context window: {err}",
                            provider.as_str(),
                        ),
                    });
                } else {
                    save_confirmation = Some(format!(
                        "Saved {} API key, model, and context window to {destination} (available in all folders)",
                        provider.as_str(),
                    ));
                }
            } else {
                save_confirmation = Some(format!(
                    "Saved {} API key and model to {destination} (available in all folders)",
                    provider.as_str(),
                ));
            }
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to save {} API key: {err}\nProvider unchanged.",
                    provider.as_str()
                ),
            });
            return false;
        }
    }

    config.provider = Some(identity.key);
    mirror_saved_api_key_in_config(config, provider, api_key);
    mirror_saved_model_in_config(config, provider, model.clone());
    if let Some(context_window) = context_window {
        mirror_saved_context_window_in_config(config, provider, context_window);
    }
    let switched = switch_provider(app, engine_handle, config, provider, Some(model)).await;
    // The switch overwrites the status line with the route summary (the full
    // summary also lands in the transcript), so the save confirmation is
    // applied last — it is the answer to the action the user just confirmed.
    if switched && let Some(confirmation) = save_confirmation {
        app.status_message = Some(confirmation);
    }
    switched
}

pub(crate) async fn apply_codewhale_owned_xai_login(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    pending: crate::xai_oauth::PendingXaiDeviceLogin,
    status_prefix: &str,
) -> bool {
    match crate::xai_oauth::activate_device_login(
        pending,
        app.config_path.as_deref(),
        Some(&mut *config),
    ) {
        Ok(activation) => {
            app.status_message = Some(format!(
                "{status_prefix}; activated {} via {}",
                codewhale_config::quote_os_path(&activation.auth_path),
                codewhale_config::quote_os_path(&activation.config_path)
            ));
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to finalize {} device login: {err:#}\nProvider unchanged.",
                    ApiProvider::Xai.as_str()
                ),
            });
            return false;
        }
    }

    switch_provider(app, engine_handle, config, ApiProvider::Xai, None).await
}

pub(crate) async fn apply_codewhale_owned_chatgpt_login(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    config: &mut Config,
    pending: crate::chatgpt_oauth::PendingChatgptPkceLogin,
    status_prefix: &str,
) -> bool {
    match crate::chatgpt_oauth::activate_pkce_login(
        pending,
        app.config_path.as_deref(),
        Some(&mut *config),
    ) {
        Ok(activation) => {
            app.status_message = Some(format!(
                "{status_prefix}; activated {} via {}",
                codewhale_config::quote_os_path(&activation.auth_path),
                codewhale_config::quote_os_path(&activation.config_path)
            ));
            app.api_key_env_only = false;
        }
        Err(err) => {
            app.add_message(HistoryCell::System {
                content: format!(
                    "Failed to finalize {} ChatGPT sign-in: {err:#}\nProvider unchanged.",
                    ApiProvider::OpenaiCodex.as_str()
                ),
            });
            return false;
        }
    }

    switch_provider(app, engine_handle, config, ApiProvider::OpenaiCodex, None).await
}

/// `/auth chatgpt-revoke`. The remote revoke is one blocking HTTP round trip
/// per stored token under the OAuth lifecycle lock, so it runs on the blocking
/// pool instead of the event loop. It targets the session's own config file
/// and clears the live route afterwards so the header stops claiming OAuth.
pub(crate) async fn run_chatgpt_revoke_from_tui(app: &mut App, config: &mut Config) {
    let config_path = app.config_path.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        crate::chatgpt_oauth::revoke_owned_login(config_path.as_deref(), None)
    })
    .await
    .map_err(|err| anyhow::anyhow!("ChatGPT revoke task was lost: {err}"))
    .and_then(|result| result);
    let message = match outcome {
        Ok(()) => {
            config.clear_codewhale_owned_chatgpt_oauth();
            "Revoked Codewhale-owned ChatGPT tokens. Codex CLI consent is unchanged.".to_string()
        }
        Err(err) => format!("ChatGPT revoke failed: {err:#}"),
    };
    app.add_message(HistoryCell::System {
        content: message.clone(),
    });
    app.status_message = Some(message);
    app.needs_redraw = true;
}

#[cfg(test)]
pub(crate) fn apply_loaded_session(
    app: &mut App,
    config: &mut Config,
    session: &SavedSession,
) -> Result<(), String> {
    apply_loaded_session_with_goal(app, config, session, None)
}

pub(crate) fn apply_loaded_session_with_goal(
    app: &mut App,
    config: &mut Config,
    session: &SavedSession,
    goal: Option<&crate::session_manager::SessionGoalState>,
) -> Result<(), String> {
    if app.session_transition_blocked() {
        return Err(
            "runtime work is active; wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work before switching sessions".to_string(),
        );
    }
    if let Some(goal) = goal {
        goal.validate()
            .map_err(|error| format!("saved session goal is invalid: {error}"))?;
    }
    let provider_identity = config.resolve_persisted_provider_identity(
        Some(&session.metadata.model_provider),
        session.metadata.model_provider_id.as_deref(),
    )?;
    let restored_route = resolve_runtime_route_for_identity(
        config,
        &provider_identity,
        Some(&session.metadata.model),
    )
    .map_err(|reason| {
        format!(
            "saved session provider '{}' could not be resolved from the live config: {reason}. Codewhale will not fall back",
            provider_identity.key
        )
    })?;
    // Restore/validate the contended state before mutating conversation or
    // workspace fields. A failed session switch must leave the current session
    // wholly intact.
    app.restore_work_state(
        &session.metadata.id,
        &session.metadata.workspace,
        session.work_state.as_ref(),
    )?;
    // All fallible preflight is complete. Retire the old session's background
    // accounting atomically before mutating live state; any late old-scope
    // provider response is rejected by `cost_status::report`.
    let _settled_old_cost_scope = crate::cost_status::close_current_scope();
    *config = *restored_route.config;
    app.api_messages = crate::runtime_handoff::project_messages_for_restore(&session.messages);
    app.clear_history();
    app.tool_cells.clear();
    app.tool_details_by_cell.clear();
    app.active_cell = None;
    app.active_tool_details.clear();
    app.active_tool_entry_completed_at.clear();
    app.active_cell_revision = app.active_cell_revision.wrapping_add(1);
    app.exploring_cell = None;
    app.exploring_entries.clear();
    app.ignored_tool_calls.clear();
    app.pending_tool_uses.clear();
    app.last_exec_wait_command = None;
    let messages = app.api_messages.clone();
    let mut message_to_cell = std::collections::HashMap::new();
    for (message_index, msg) in messages.iter().enumerate() {
        let mut cells = history_cells_from_message(msg);
        if msg.role == "user"
            && session
                .context_references
                .iter()
                .any(|record| record.message_index == message_index)
        {
            for cell in &mut cells {
                if let HistoryCell::User { content } = cell {
                    *content = compact_user_context_display(content);
                }
            }
        }
        let base = app.history.len();
        if msg.role == "user"
            && let Some(offset) = cells
                .iter()
                .position(|cell| matches!(cell, HistoryCell::User { .. }))
        {
            message_to_cell.insert(message_index, base + offset);
        }
        app.extend_history(cells);
    }
    app.rebuild_completed_assistant_outputs_from_restored_history();
    app.sync_context_references_from_session(&session.context_references, &message_to_cell);
    app.mark_history_updated();
    app.viewport.transcript_selection.clear();
    // Goal state is session-owned just like Work state. A legacy/no-goal
    // session clears the previous session's objective; a durable sidecar
    // rebuilds both the visible hunt and the EngineConfig seeded below.
    app.goal = crate::tui::app::HostGoalState::default();
    app.last_known_goal_state = None;
    app.pending_goal_controls.clear();
    if let Some(goal) = goal {
        let snapshot = goal.to_runtime_snapshot();
        let _ = apply_goal_snapshot_to_app(app, &snapshot);
    }
    restore_loaded_session_provider(app, config, provider_identity);
    // Session records do not own a reasoning preference. `set_model_selection`
    // restores the raw explicit global preference for Auto (or releases an
    // implicit fixed-route default) instead of reusing normalized live state.
    app.set_model_selection(session.metadata.model.clone());
    if app.auto_model
        && let Some(saved) = session.last_auto_route.as_ref()
        && !saved.provider_identity.trim().is_empty()
        && !saved.model.trim().is_empty()
    {
        app.last_effective_provider = Some(saved.provider);
        app.last_effective_provider_identity = Some(saved.provider_identity.clone());
        app.last_effective_model = Some(saved.model.clone());
        app.last_auto_route_receipt = Some(saved.receipt.clone());
        app.last_effective_reasoning_effort = saved.effective_reasoning_effort.map(Into::into);
    }
    resolve_loaded_session_route(app, config);
    if !app.auto_model {
        let requested = app
            .reasoning_effort_preference
            .unwrap_or(app.reasoning_effort);
        app.reasoning_effort =
            requested.normalize_for_route(app.api_provider, &app.active_route_base_url, &app.model);
    }
    app.provider_models.insert(
        app.provider_identity_for_persistence().to_string(),
        app.model_selection_for_persistence(),
    );
    app.update_model_compaction_budget();
    apply_workspace_runtime_state(app, config, session.metadata.workspace.clone());
    if let Some(mode) = session.metadata.mode.as_deref().and_then(AppMode::parse) {
        app.set_mode(mode);
    }
    app.session.total_tokens = u32::try_from(session.metadata.total_tokens).unwrap_or(u32::MAX);
    app.session.total_conversation_tokens = app.session.total_tokens;
    let restored_parent = crate::pricing::CostEstimate {
        usd: session.metadata.cost.session_cost_usd,
        cny: session.metadata.cost.session_cost_cny,
    }
    .sanitized();
    let restored_background = crate::pricing::CostEstimate {
        usd: session.metadata.cost.subagent_cost_usd,
        cny: session.metadata.cost.subagent_cost_cny,
    }
    .sanitized();
    // A restored session has no live billed receipt; the estimate rules the
    // meter until the next model call reports one.
    app.last_billed_input_tokens = None;
    app.session.session_cost = restored_parent.usd;
    app.session.session_cost_cny = restored_parent.cny;
    app.session.subagent_cost = restored_background.usd;
    app.session.subagent_cost_cny = restored_background.cny;
    app.session.subagent_usage_sources = session
        .metadata
        .cost
        .usage_source_fingerprints
        .iter()
        .cloned()
        .collect();
    crate::cost_status::restore_usage_source_fingerprints(
        session
            .metadata
            .cost
            .usage_source_fingerprints
            .iter()
            .cloned(),
    );
    // Coverage is restored *with* the money, and the live counters are cleared
    // first: whatever the previous session in this process priced is not inside
    // the total being loaded, so carrying those counters over would describe the
    // wrong total (#4318).
    app.reset_cost_coverage();
    app.session.cost_priced_turns = session.metadata.cost.priced_turns;
    app.session.cost_unpriced_turns = session.metadata.cost.unpriced_turns;
    app.session.cost_cny_priced_turns = session.metadata.cost.cny_priced_turns;
    app.session.cost_cny_unpriced_turns = session.metadata.cost.cny_unpriced_turns;
    app.session.cost_unpriced_reasons = session.metadata.cost.unpriced_reasons.clone();
    app.session.cost_cny_unpriced_reasons = session.metadata.cost.cny_unpriced_reasons.clone();
    app.session.cost_unpriced_classes = session.metadata.cost.unpriced_classes.clone();
    app.session.cost_pricing_provenances = session.metadata.cost.pricing_provenances.clone();
    app.session.cost_live_pricing_defects = session.metadata.cost.live_pricing_defects.clone();
    app.session.cost_live_pricing_unusable_defects =
        session.metadata.cost.live_pricing_unusable_defects.clone();
    app.session.cost_route_receipts = session.metadata.cost.route_receipts.clone();
    // A pre-coverage session deserializes its new fields from serde defaults,
    // which are indistinguishable from "complete total, zero turns". Flag it so
    // `/cost` says the coverage is unknown rather than claiming completeness,
    // including for an all-zero record.
    app.session.cost_coverage_unknown_legacy = session.metadata.cost.coverage_is_legacy_unknown();
    // Restore the high-water marks from persisted metadata so the
    // monotonic cost guarantee (#244) survives session restarts.
    // Take the max with the current totals — old sessions without
    // persisted high-water fields deserialise to 0.0 and fall back to
    // the restored total with no regression.
    let total_restored_usd = session.metadata.cost.total_usd();
    let total_restored_cny = session.metadata.cost.total_cny();
    let restored_high_water = crate::pricing::CostEstimate {
        usd: session.metadata.cost.displayed_cost_high_water_usd,
        cny: session.metadata.cost.displayed_cost_high_water_cny,
    }
    .sanitized();
    app.session.displayed_cost_high_water = restored_high_water.usd.max(total_restored_usd);
    app.session.displayed_cost_high_water_cny = restored_high_water.cny.max(total_restored_cny);
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_output_throughput = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    // Accumulated token breakdown is per-runtime-session; reset on load.
    app.session.reset_token_breakdown();
    // The metrics strip shares that scope: it describes this runtime
    // session's calls, not the restored transcript's.
    app.session_metrics = crate::tui::session_metrics::SessionMetrics::default();
    app.session.turn_cache_history.clear();
    // Restore cumulative turn duration so the footer "worked" chip
    // persists across session restarts (#2038).
    app.cumulative_turn_duration =
        std::time::Duration::from_secs(session.metadata.cumulative_turn_secs);
    app.current_session_id = Some(session.metadata.id.clone());
    app.current_session_metadata = Some(session.metadata.clone());
    app.session_artifacts = session.artifacts.clone();
    app.session_title = Some(session.metadata.title.clone());
    app.window_title = session.window_title.clone();
    app.workspace_context = None;
    app.workspace_context_refreshed_at = None;
    if let Some(sp) = session.system_prompt.as_ref() {
        app.system_prompt = Some(SystemPrompt::Text(sp.clone()));
    } else {
        app.system_prompt = None;
    }
    app.scroll_to_bottom();
    Ok(())
}

pub(crate) fn apply_loaded_session_config_snapshot(
    app: &mut App,
    config: &mut Config,
    session: &SavedSession,
    mut next_config: Config,
    force_engine_respawn: bool,
) -> Result<bool, String> {
    if force_engine_respawn {
        // File `/load` supplies a freshly loaded disk snapshot, but the live
        // Config also contains CLI and workspace/project overlays that are not
        // represented by that file. Refresh the provider registry atomically
        // over the effective Config instead of dropping permission controls.
        let mut effective_config = config.clone();
        effective_config.refresh_provider_routes_from(&next_config);
        next_config = effective_config;
    }
    let previous_provider = app.api_provider;
    let previous_provider_identity = app.provider_identity_for_persistence().to_string();
    let previous_workspace = app.workspace.clone();
    let goal = SessionManager::default_location()
        .and_then(|manager| manager.load_session_goal(&session.metadata.id))
        .map_err(|error| format!("saved session goal could not be loaded: {error}"))?;
    apply_loaded_session_with_goal(app, &mut next_config, session, goal.as_ref())?;
    // A file load reads a fresh disk snapshot. Even when the route's enum and
    // exact identity are unchanged, endpoint, key, headers, TLS, or retry
    // settings may have changed. Rebuild from that same validated snapshot so
    // compaction and other pre-turn engine work cannot retain the old client.
    let respawn = force_engine_respawn
        || loaded_session_requires_engine_respawn(
            app,
            previous_provider,
            &previous_provider_identity,
            &previous_workspace,
        );
    *config = next_config;
    Ok(respawn)
}
