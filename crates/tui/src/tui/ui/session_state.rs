//! Session durability: snapshot/restore, recovery after a crash or stall,
//! and workspace/worktree switching.
//!
//! Moved verbatim out of `ui.rs`.

use super::*;

pub(crate) async fn publish_pending_work_projection(app: &mut App) -> Result<bool, String> {
    let Some(work) = app.runtime_services.work.clone() else {
        return Ok(false);
    };
    let published = work.publish_pending().await?;
    Ok(published)
}

pub(crate) async fn persist_pending_work_checkpoint(app: &mut App) -> Result<bool, String> {
    let Some(work) = app.runtime_services.work.clone() else {
        return Ok(false);
    };
    if !work.has_pending_publish() {
        return Ok(false);
    }
    let manager = SessionManager::default_location()
        .map_err(|err| format!("could not open sessions directory: {err}"))?;
    let session = build_session_snapshot(app, &manager)?;
    if app.current_session_id.is_none() {
        app.current_session_id = Some(session.metadata.id.clone());
    }
    if !persistence_actor::try_persist(PersistRequest::SaveCheckpoint { session }) {
        return Err("persistence actor is unavailable".to_string());
    }
    publish_pending_work_projection(app).await
}

pub(crate) fn persist_with_pending_work_boundary(
    app: &mut App,
    request: PersistRequest,
) -> Result<(), String> {
    let has_pending = app
        .runtime_services
        .work
        .as_ref()
        .is_some_and(|work| work.has_pending_publish());
    if !has_pending {
        persistence_actor::persist(request);
        return Ok(());
    }
    if !persistence_actor::try_persist(request) {
        return Err("persistence actor is unavailable".to_string());
    }
    app.publish_pending_work_state().map(|_| ())
}

pub(crate) fn restore_matching_offline_queue_state(
    app: &mut App,
    state: OfflineQueueState,
) -> bool {
    if state.session_id.as_deref() != app.current_session_id.as_deref()
        || state.session_id.is_none()
    {
        return false;
    }
    app.queued_messages = state
        .messages
        .into_iter()
        .map(queued_session_to_ui)
        .collect();
    if let Some(draft) = state.draft.map(queued_session_to_ui) {
        app.input.clone_from(&draft.display);
        app.cursor_position = app.input.chars().count();
        app.active_skill.clone_from(&draft.skill_instruction);
        app.active_skill_provenance
            .clone_from(&draft.skill_provenance);
        app.queued_draft = Some(draft);
    } else {
        app.queued_draft = None;
    }
    app.needs_redraw = true;
    true
}

pub(crate) fn reconcile_turn_liveness(
    app: &mut App,
    now: Instant,
    has_running_agents: bool,
) -> bool {
    if app.is_loading
        && app.runtime_turn_status.is_none()
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
        && app.dispatch_started_at.is_some_and(|started| {
            now.saturating_duration_since(started) > DISPATCH_WATCHDOG_TIMEOUT
        })
    {
        // #2739: the user's prompt was already appended to api_messages
        // before dispatch, but the turn never reached `in_progress`. Persist
        // it before clearing turn state so `--continue` keeps the prompt
        // instead of loading the previous save.
        persist_recovery_snapshot(app);
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.turn_started_at = None;
        app.turn_last_activity_at = None;
        app.pending_turn_route = None;
        app.pending_auto_route_receipt = None;
        app.active_turn = None;
        app.suppress_stream_events_until_turn_complete = false;
        app.push_status_toast(
            "Turn dispatch timed out; the engine may have stopped. Please try again.",
            StatusToastLevel::Error,
            None,
        );
        return true;
    }

    if app.is_loading
        && matches!(
            app.runtime_turn_status.as_deref(),
            Some("completed" | "interrupted" | "failed")
        )
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
    {
        app.is_loading = false;
        app.dispatch_started_at = None;
        app.turn_started_at = None;
        app.turn_last_activity_at = None;
        app.pending_turn_route = None;
        app.pending_auto_route_receipt = None;
        app.active_turn = None;
        app.suppress_stream_events_until_turn_complete = false;
        app.push_status_toast(
            "Recovered from an inconsistent busy state.",
            StatusToastLevel::Warning,
            None,
        );
        return true;
    }

    // Branch 3: turn started but never completed — engine may have
    // panicked, sub-agent may be stuck, or the completion event was lost.
    if app.is_loading
        && matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        && !has_running_agents
        && !app.is_compacting
        && !active_turn_has_running_tool(app)
        && app
            .turn_last_activity_at
            .or(app.turn_started_at)
            .is_some_and(|last_activity| {
                now.saturating_duration_since(last_activity) > turn_stall_watchdog_timeout(app)
            })
    {
        recover_stalled_runtime_turn(
            app,
            "Turn stalled — no completion signal received. Please try again.",
            StatusToastLevel::Error,
        );
        return true;
    }

    if app.is_loading
        && matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        && !has_running_agents
        && !app.is_compacting
        && !app.is_purging
        && active_turn_has_running_tool(app)
        && app
            .turn_last_activity_at
            .or(app.turn_started_at)
            .is_some_and(|last_activity| {
                now.saturating_duration_since(last_activity) > TOOL_HANG_WATCHDOG_TIMEOUT
            })
    {
        recover_stalled_runtime_turn(
            app,
            "Tool stalled with no progress for 10m — recovered; the command may still be running in the background. Use exec_shell_cancel or retry.",
            StatusToastLevel::Error,
        );
        return true;
    }

    false
}

/// #2739: persist the current in-memory session state before a recovery or
/// cancellation path clears turn bookkeeping. Without this snapshot, the
/// just-finalised partial turn lives only in `app.api_messages` and is never
/// written to disk, so `--continue` loads the *previous* save — effectively
/// losing the entire in-progress turn.
pub(crate) fn persist_recovery_snapshot(app: &mut App) {
    if let Ok(manager) = SessionManager::default_location()
        && let Ok(session) = build_session_snapshot(app, &manager)
    {
        if app.current_session_id.is_none() {
            app.current_session_id = Some(session.metadata.id.clone());
        }
        if let Err(err) =
            persist_with_pending_work_boundary(app, PersistRequest::SaveCheckpoint { session })
        {
            app.status_message = Some(format!(
                "To-do list update pending: recovery snapshot could not be queued ({err})"
            ));
        }
    }
}

pub(crate) fn persist_full_reset_snapshot(app: &mut App) {
    if let Ok(manager) = SessionManager::default_location()
        && let Ok(session) = build_session_snapshot(app, &manager)
    {
        app.current_session_id = Some(session.metadata.id.clone());
        if let Err(err) =
            persist_with_pending_work_boundary(app, PersistRequest::SessionSnapshot(session))
        {
            app.status_message = Some(format!(
                "To-do list update pending: reset snapshot could not be queued ({err})"
            ));
        }
    }
    // `/clear` and `/new` are explicit boundaries. Never let an older
    // in-flight checkpoint resurrect the session the user just discarded,
    // even if the replacement snapshot could not be constructed.
    // `build_session_snapshot` reuses `current_session_id`, so this id is the
    // discarded session's id whether or not the snapshot above succeeded.
    if let Some(session_id) = app.current_session_id.clone() {
        persistence_actor::persist(PersistRequest::ClearCheckpoint { session_id });
    }
}

pub(crate) fn maybe_throttled_recovery_snapshot(
    app: &mut App,
    now: Instant,
    last_snapshot_at: &mut Option<Instant>,
) {
    if !app.is_loading && !matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        return;
    }
    if last_snapshot_at
        .is_some_and(|last| now.saturating_duration_since(last) < RECOVERY_SNAPSHOT_INTERVAL)
    {
        return;
    }
    persist_recovery_snapshot(app);
    *last_snapshot_at = Some(now);
}

pub(crate) fn recover_stalled_runtime_turn(app: &mut App, message: &str, level: StatusToastLevel) {
    // Capture the turn identity before the reset below clears it; the
    // outbox event must name the turn that stalled.
    let stalled_turn_id = app.runtime_turn_id.clone();
    let stalled_session_id = app.hooks.session_id().to_string();
    // Finalize in-flight thinking / assistant / tool cells so the
    // transcript doesn't show permanent spinners after recovery.
    streaming_thinking::finalize_current(app);
    app.finalize_streaming_assistant_as_interrupted();
    app.finalize_active_cell_as_interrupted();
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #2739: persist the partial turn's api_messages before clearing
    // turn state. Without this snapshot the stalled/cancelled turn's
    // messages are held only in memory and --continue sees the
    // *previous* save, losing the entire in-progress turn.
    persist_recovery_snapshot(app);

    app.is_loading = false;
    app.turn_started_at = None;
    app.turn_last_activity_at = None;
    app.runtime_turn_status = None;
    app.runtime_turn_id = None;
    app.dispatch_started_at = None;
    app.pending_turn_route = None;
    app.pending_auto_route_receipt = None;
    app.active_turn = None;
    app.suppress_stream_events_until_turn_complete = false;
    // Per-turn scroll lock — clear so the next turn auto-scrolls.
    app.user_scrolled_during_stream = false;
    app.push_status_toast(message, level, None);
    // Lifecycle outbox (`[lifecycle_outbox]`): the first scriptable stall
    // signal. Until now a wedged turn was only visible as this toast; with
    // the outbox enabled a supervisor can react to the same moment.
    // No-op when the feature is disabled.
    app.lifecycle_outbox.emit(codewhale_hooks::LifecycleEvent {
        event: "turn_stalled".to_string(),
        kind: "turn.stalled".to_string(),
        thread_id: stalled_session_id,
        turn_id: stalled_turn_id,
        item_id: None,
        payload: serde_json::json!({
            "message": codewhale_hooks::bounded_text(
                message,
                codewhale_hooks::OUTBOX_DETAIL_MAX_CHARS,
            ),
            "workspace": app.workspace.display().to_string(),
        }),
    });
}

pub(crate) fn recover_engine_event_disconnect(app: &mut App) -> bool {
    let had_live_work = app.is_loading
        || app.is_compacting
        || app.manual_compaction_queued
        || app.is_purging
        || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
        || app.pending_turn_route.is_some()
        || app.active_turn.is_some()
        || app.suppress_stream_events_until_turn_complete
        || app.streaming_message_index.is_some()
        || app.streaming_thinking_active_entry.is_some()
        || app
            .active_cell
            .as_ref()
            .is_some_and(|cell| !cell.is_empty());

    if !had_live_work {
        return false;
    }

    streaming_thinking::finalize_current(app);
    app.finalize_streaming_assistant_as_interrupted();
    app.finalize_active_cell_as_interrupted();
    app.streaming_state.reset();
    app.streaming_message_index = None;
    app.streaming_thinking_active_entry = None;

    // #2739: persist partial turn before clearing state.
    persist_recovery_snapshot(app);

    app.is_loading = false;
    app.is_compacting = false;
    app.active_compaction = None;
    app.manual_compaction_queued = false;
    app.deferred_manual_compaction = None;
    app.is_purging = false;
    app.turn_started_at = None;
    app.turn_last_activity_at = None;
    app.runtime_turn_status = None;
    app.runtime_turn_id = None;
    app.dispatch_started_at = None;
    app.pending_turn_route = None;
    app.pending_auto_route_receipt = None;
    app.active_turn = None;
    app.suppress_stream_events_until_turn_complete = false;
    app.user_scrolled_during_stream = false;

    for msg in app.drain_pending_steers() {
        app.queue_message(msg);
    }

    app.add_message(HistoryCell::Error {
        message: "Engine stopped before completing the turn. Check ~/.codewhale/crashes and retry."
            .to_string(),
        severity: crate::error_taxonomy::ErrorSeverity::Error,
    });
    app.push_status_toast(
        "Engine stopped before completing the turn.",
        StatusToastLevel::Error,
        None,
    );
    true
}

pub(crate) fn capture_turn_started_metadata(app: &mut App, event: &EngineEvent) {
    match event {
        EngineEvent::TurnStarted {
            turn_id,
            created_at,
            route,
        } => {
            app.ocean_completion_started_at = None;
            let auto_route_receipt = if route.as_ref().is_some_and(|route| route.auto_model) {
                app.pending_auto_route_receipt.take()
            } else if route.is_some() {
                app.pending_auto_route_receipt = None;
                None
            } else {
                None
            };
            // Bind the prompt-suggestion authority to the receipt the engine minted
            // from the client it installed for this turn. Deliberately not read
            // from `config`: web config events are drained ahead of engine events,
            // so config here may already describe a different key or endpoint than
            // the one this turn is actually running on.
            let suggestion_authority = route
                .as_ref()
                .and_then(crate::tui::prompt_suggestion::capture_route_authority);
            app.active_turn = Some(ActiveTurnMetadata {
                turn_id: turn_id.clone(),
                created_at: *created_at,
                route: route.clone(),
                auto_route_receipt,
                suggestion_authority,
            });
            app.pending_turn_route = None;
        }
        // The dispatch boundary is the billing truth: refresh the active turn's
        // route with the envelope that was actually put on the wire. Receipts
        // already taken at `TurnStarted` are preserved — this event narrows the
        // route, it never re-opens an authority decision.
        EngineEvent::RouteDispatched { turn_id, route } => {
            if let Some(active) = app
                .active_turn
                .as_mut()
                .filter(|active| active.turn_id == *turn_id)
            {
                if route.auto_model && active.auto_route_receipt.is_none() {
                    active.auto_route_receipt = app.pending_auto_route_receipt.take();
                } else if !route.auto_model {
                    app.pending_auto_route_receipt = None;
                    active.auto_route_receipt = None;
                }
                if active.suggestion_authority.is_none() {
                    active.suggestion_authority =
                        crate::tui::prompt_suggestion::capture_route_authority(route);
                }
                active.route = Some(route.clone());
            }
        }
        _ => {}
    }
}

pub(crate) fn record_turn_activity(app: &mut App, event: &EngineEvent, now: Instant) {
    if matches!(event, EngineEvent::TurnStarted { .. }) {
        app.turn_last_activity_at = Some(now);
        return;
    }

    if app.is_loading || matches!(app.runtime_turn_status.as_deref(), Some("in_progress")) {
        app.turn_last_activity_at = Some(now);
    }
}

pub(crate) fn persist_offline_queue_state(app: &App) {
    if app.queued_messages.is_empty() && app.queued_draft.is_none() {
        persistence_actor::persist(PersistRequest::ClearOfflineQueue);
        return;
    }
    let state = OfflineQueueState {
        messages: app
            .queued_messages
            .iter()
            .map(queued_ui_to_session)
            .collect(),
        draft: app.queued_draft.as_ref().map(queued_ui_to_session),
        ..OfflineQueueState::default()
    };
    persistence_actor::persist(PersistRequest::OfflineQueue {
        state,
        session_id: app.current_session_id.clone(),
    });
}

pub(crate) fn restore_queued_message(app: &mut App, index: Option<usize>, message: QueuedMessage) {
    if let Some(index) = index
        && index <= app.queued_messages.len()
    {
        app.queued_messages.insert(index, message);
    } else {
        app.queue_message(message);
    }
}

pub(crate) fn restore_queued_or_draft_message(
    app: &mut App,
    recovery: DispatchRecovery,
    message: QueuedMessage,
) {
    match recovery {
        DispatchRecovery::Draft => {
            app.input.clone_from(&message.display);
            app.cursor_position = app.input.chars().count();
            app.active_skill = message.skill_instruction.clone();
            app.active_skill_provenance = message.skill_provenance.clone();
            app.queued_draft = Some(message);
            app.needs_redraw = true;
        }
        DispatchRecovery::Queued { restore_index } => {
            restore_queued_message(app, restore_index, message);
        }
        DispatchRecovery::Immediate | DispatchRecovery::Initial => app.queue_message(message),
    }
}

pub(crate) fn recover_unstarted_external_message(
    app: &mut App,
    message: QueuedMessage,
    recovery: DispatchRecovery,
    error: &str,
) {
    app.dispatch_in_flight = false;
    match recovery {
        DispatchRecovery::Immediate | DispatchRecovery::Initial => {
            restore_failed_immediate_submit(app, message, &anyhow::Error::msg(error.to_string()));
        }
        DispatchRecovery::Draft => {
            restore_queued_or_draft_message(app, recovery, message);
            app.status_message = Some(format!("{error}; queued draft restored"));
        }
        DispatchRecovery::Queued { restore_index } => {
            restore_queued_message(app, restore_index, message);
            app.status_message = Some(format!(
                "{error}; {} queued follow-up(s) restored",
                app.queued_message_count()
            ));
        }
    }
    app.push_status_toast(
        error.to_string(),
        StatusToastLevel::Error,
        Some(App::STICKY_ERROR_TTL_MS),
    );
    app.needs_redraw = true;
}

pub(crate) fn restore_message_submit_denial(
    app: &mut App,
    message: QueuedMessage,
    recovery: DispatchRecovery,
) {
    let denial = app
        .status_message
        .clone()
        .unwrap_or_else(|| "message_submit hook blocked submission".to_string());
    app.dispatch_in_flight = false;
    match recovery {
        DispatchRecovery::Immediate | DispatchRecovery::Initial => {
            app.input.clone_from(&message.display);
            app.cursor_position = app.input.chars().count();
            app.active_skill = message.skill_instruction;
            app.active_skill_provenance = message.skill_provenance;
        }
        DispatchRecovery::Draft => {
            restore_queued_or_draft_message(app, recovery, message);
        }
        DispatchRecovery::Queued { restore_index } => {
            restore_queued_message(app, restore_index, message);
        }
    }
    app.status_message = Some(denial.clone());
    app.push_status_toast(denial, StatusToastLevel::Warning, Some(6_000));
    app.needs_redraw = true;
}

/// Resume one recent-work row from the startup card by session id. Mirrors
/// `/resume <id>`: the card dissolves and the saved session loads through
/// the normal `LoadSession` path; a session that vanished behind the card
/// leaves the card up with a status saying why instead of stranding the
/// user on an empty stage.
pub(crate) fn resume_launch_session(app: &mut App, session_id: &str) -> commands::CommandResult {
    let failed = |app: &mut App, err: &str| {
        app.launch.status = Some(
            app.tr(MessageId::LaunchResumeFailed)
                .replace("{error}", err),
        );
        commands::CommandResult::ok()
    };
    let manager = match crate::session_manager::SessionManager::default_location() {
        Ok(manager) => manager,
        Err(err) => return failed(app, &err.to_string()),
    };
    let saved = match manager.load_session(session_id) {
        Ok(saved) => saved,
        Err(err) => return failed(app, &err.to_string()),
    };
    let path = manager
        .sessions_dir()
        .join(format!("{}.json", saved.metadata.id));
    if !path.exists() {
        return failed(app, "saved session file is gone");
    }
    app.launch.dissolve_card(app.ambient_clock_ms);
    commands::CommandResult::action(AppAction::LoadSession(path))
}

pub(crate) fn begin_launch_session(
    app: &mut App,
    workspace: Option<PathBuf>,
) -> commands::CommandResult {
    if let Some(workspace) = workspace {
        app.workspace = workspace;
    }
    let session_id = uuid::Uuid::new_v4().to_string();
    app.current_session_id = Some(session_id.clone());
    app.current_session_metadata = None;
    app.session_title = Some(app.tr(MessageId::SessionsNewSessionTitle).into_owned());
    app.launch.visible = false;
    app.launch.status = None;
    app.status_message = None;
    commands::CommandResult::action(AppAction::SyncSession {
        session_id: Some(session_id),
        messages: Vec::new(),
        system_prompt: None,
        model: app.model.clone(),
        workspace: app.workspace.clone(),
        mode: app.mode,
    })
}

pub(crate) async fn sync_runtime_workspace_state(
    task_manager: &SharedTaskManager,
    workspace: PathBuf,
) {
    task_manager.set_default_workspace(workspace).await;
}

pub(crate) async fn switch_workspace(
    app: &mut App,
    engine_handle: &mut EngineHandle,
    task_manager: &SharedTaskManager,
    config: &Config,
    workspace: PathBuf,
) {
    if app.is_loading {
        app.status_message =
            Some("Cannot switch workspace while a request is running.".to_string());
        app.add_message(HistoryCell::System {
            content: "Cannot switch workspace while a request is running.".to_string(),
        });
        return;
    }

    if app.workspace == workspace {
        app.status_message = Some(format!("Workspace unchanged: {}", workspace.display()));
        return;
    }

    apply_workspace_runtime_state(app, config, workspace.clone());
    sync_runtime_workspace_state(task_manager, workspace.clone()).await;

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
                workspace: workspace.clone(),
                mode: app.mode,
            })
            .await;
    }

    app.add_message(HistoryCell::System {
        content: format!("Switched workspace to {}", workspace.display()),
    });
    app.status_message = Some(format!("Workspace: {}", workspace.display()));
}

pub(crate) fn restore_failed_immediate_submit(
    app: &mut App,
    message: QueuedMessage,
    error: &anyhow::Error,
) {
    tracing::warn!(
        error = %error,
        "immediate user message dispatch failed; restored composer"
    );
    app.input = message.display;
    app.cursor_position = app.input.chars().count();
    app.active_skill = message.skill_instruction;
    app.active_skill_provenance = message.skill_provenance;
    let status = tr(app.ui_locale, MessageId::ComposerDispatchFailedRestored)
        .replace("{error}", &error.to_string());
    app.status_message = Some(status.clone());
    app.set_sticky_status(
        status,
        StatusToastLevel::Error,
        Some(App::STICKY_ERROR_TTL_MS),
    );
    app.needs_redraw = true;
}

/// Show the default recommended Hotbar slots. Since #3807 an absent `hotbar`
/// key means "hidden", so `/hotbar on` persists the explicit default bindings
/// rather than deleting the key. This is an explicit reset, so any custom
/// bindings are replaced with the recommended set.
pub(crate) fn restore_hotbar_defaults(app: &mut App, config: &mut Config) {
    let defaults = codewhale_config::default_hotbar_bindings_toml();
    match crate::config_persistence::persist_hotbar_bindings(app.config_path.as_deref(), &defaults)
    {
        Ok(path) => {
            config.hotbar = Some(defaults);
            app.status_message = Some(format!(
                "Hotbar enabled with the default slots ({}). Customize with `/hotbar`.",
                path.display()
            ));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to enable the Hotbar: {err}"));
            app.add_message(HistoryCell::System {
                content: format!("Failed to enable the Hotbar: {err}"),
            });
        }
    }
    app.needs_redraw = true;
}

pub(crate) fn persist_rules_from_approval(
    app: &mut App,
    config: &mut Config,
    rules: &[codewhale_config::ToolAskRule],
) {
    let action = rules.first().map(|rule| rule.action);
    match codewhale_config::ConfigStore::load(app.config_path.clone()).and_then(|mut store| {
        let added = match action {
            Some(codewhale_execpolicy::PermissionAction::Ask) => store.append_ask_rules(rules)?,
            Some(codewhale_execpolicy::PermissionAction::Allow) => {
                store.append_allow_rules(rules)?
            }
            Some(codewhale_execpolicy::PermissionAction::Deny) => {
                anyhow::bail!("the approval UI cannot persist deny rules")
            }
            None => 0,
        };
        let permissions_path = store.permissions_path();
        config.exec_policy_engine = store.exec_policy_engine();
        Ok((added, permissions_path))
    }) {
        Ok((added, path)) if added > 0 => {
            let action = match action {
                Some(codewhale_execpolicy::PermissionAction::Allow) => "allow",
                _ => "ask",
            };
            app.status_message = Some(format!(
                "Saved {added} {action} permission rule(s) to {}",
                path.display()
            ));
        }
        Ok((_added, path)) => {
            let action = match action {
                Some(codewhale_execpolicy::PermissionAction::Allow) => "Allow",
                _ => "Ask",
            };
            app.status_message = Some(format!(
                "{action} permission rule already saved in {}",
                path.display()
            ));
        }
        Err(err) => {
            app.status_message = Some(format!("Failed to save permission rule: {err:#}"));
        }
    }
}

pub(crate) fn mirror_saved_model_in_config(
    config: &mut Config,
    provider: ApiProvider,
    model: String,
) {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        config.default_text_model = Some(model);
        return;
    }
    config.set_provider_model_override(provider, Some(model));
}

pub(crate) fn mirror_saved_context_window_in_config(
    config: &mut Config,
    provider: ApiProvider,
    context_window: u32,
) {
    let providers = config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry = match provider {
        ApiProvider::Moonshot => &mut providers.moonshot,
        _ => return,
    };
    entry.context_window = Some(context_window);
}

pub(crate) fn mirror_saved_api_key_in_config(
    config: &mut Config,
    provider: ApiProvider,
    api_key: String,
) {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        config.api_key = Some(api_key);
        config.auth_mode = Some("api_key".to_string());
        return;
    }
    if provider == ApiProvider::Custom && config.uses_legacy_literal_custom_route() {
        config.api_key = Some(api_key);
        config.auth_mode = Some("api_key".to_string());
        return;
    }
    let pin_kimi_code_base_url = provider == ApiProvider::Moonshot
        && config.provider_config_for(provider).is_some_and(|entry| {
            crate::config::provider_config_uses_kimi_imported_token(entry)
                && entry
                    .base_url
                    .as_deref()
                    .is_none_or(|base_url| base_url.trim().is_empty())
        });
    let custom_key = (provider == ApiProvider::Custom).then(|| {
        config
            .provider
            .clone()
            .unwrap_or_else(|| "__custom__".to_string())
    });
    let providers = config
        .providers
        .get_or_insert_with(ProvidersConfig::default);
    let entry: &mut ProviderConfig = match provider {
        ApiProvider::Deepseek | ApiProvider::DeepseekCN => return,
        ApiProvider::Custom => providers
            .custom
            .entry(custom_key.expect("custom key captured for custom provider"))
            .or_default(),
        ApiProvider::DeepseekAnthropic => &mut providers.deepseek_anthropic,
        ApiProvider::NvidiaNim => &mut providers.nvidia_nim,
        ApiProvider::Openai => &mut providers.openai,
        ApiProvider::Atlascloud => &mut providers.atlascloud,
        ApiProvider::WanjieArk => &mut providers.wanjie_ark,
        ApiProvider::Volcengine => &mut providers.volcengine,
        ApiProvider::Openrouter => &mut providers.openrouter,
        ApiProvider::Orcarouter => &mut providers.orcarouter,
        ApiProvider::XiaomiMimo => &mut providers.xiaomi_mimo,
        ApiProvider::Novita => &mut providers.novita,
        ApiProvider::Fireworks => &mut providers.fireworks,
        ApiProvider::Siliconflow | ApiProvider::SiliconflowCn => &mut providers.siliconflow,
        ApiProvider::Arcee => &mut providers.arcee,
        ApiProvider::Moonshot => &mut providers.moonshot,
        ApiProvider::Sglang => &mut providers.sglang,
        ApiProvider::Vllm => &mut providers.vllm,
        ApiProvider::Ollama => &mut providers.ollama,
        ApiProvider::OllamaCloud => &mut providers.ollama_cloud,
        ApiProvider::Huggingface => &mut providers.huggingface,
        ApiProvider::Deepinfra => &mut providers.deepinfra,
        ApiProvider::Together => &mut providers.together,
        ApiProvider::Qianfan => &mut providers.qianfan,
        ApiProvider::OpenaiCodex => &mut providers.openai_codex,
        ApiProvider::Anthropic => &mut providers.anthropic,
        ApiProvider::Openmodel => &mut providers.openmodel,
        ApiProvider::Zai => &mut providers.zai,
        ApiProvider::Stepfun => &mut providers.stepfun,
        ApiProvider::Minimax => &mut providers.minimax,
        ApiProvider::MinimaxAnthropic => &mut providers.minimax_anthropic,
        ApiProvider::Sakana => &mut providers.sakana,
        ApiProvider::LongCat => &mut providers.longcat,
        ApiProvider::OpencodeGo => &mut providers.opencode_go,
        ApiProvider::OpencodeZen => &mut providers.opencode_zen,
        ApiProvider::Meta => &mut providers.meta,
        ApiProvider::Xai => &mut providers.xai,
        ApiProvider::Mistral => &mut providers.mistral,
        ApiProvider::Google => &mut providers.google,
        ApiProvider::Antigravity => &mut providers.antigravity,
        ApiProvider::Telecomjs => &mut providers.telecomjs,
        ApiProvider::Edenai => &mut providers.edenai,
        ApiProvider::Concentrate => &mut providers.concentrate,
        ApiProvider::ModelstudioTokenPlan => &mut providers.modelstudio_token_plan,
        ApiProvider::ModelstudioTokenPlanAnthropic => {
            &mut providers.modelstudio_token_plan_anthropic
        }
        ApiProvider::ModelstudioCodingPlan => &mut providers.modelstudio_coding_plan,
        ApiProvider::ModelstudioCodingPlanAnthropic => {
            &mut providers.modelstudio_coding_plan_anthropic
        }
    };
    if pin_kimi_code_base_url {
        entry.base_url = Some(crate::config::DEFAULT_KIMI_CODE_BASE_URL.to_string());
    }
    entry.auth_mode = Some("api_key".to_string());
    entry.api_key = Some(api_key);
    entry.external_credentials = None;
    if provider == ApiProvider::Xai {
        entry.oauth_credential_generation = None;
    }
}

pub(crate) fn loaded_session_requires_engine_respawn(
    app: &App,
    previous_provider: ApiProvider,
    previous_provider_identity: &str,
    previous_workspace: &Path,
) -> bool {
    app.api_provider != previous_provider
        || app.provider_identity_for_persistence() != previous_provider_identity
        || app.workspace != previous_workspace
}

pub(crate) fn restore_loaded_session_provider(
    app: &mut App,
    config: &mut Config,
    identity: ProviderIdentity,
) {
    let provider = identity.provider;
    config.scope_to_provider_identity(&identity);
    app.set_provider_identity_record(identity);
    app.billing_presentation = crate::route_billing::for_route(config, provider);
    app.max_subagents = config
        .max_subagents_for_provider(provider)
        .clamp(1, crate::config::MAX_SUBAGENTS);
    app.provider_chain = provider
        .kind()
        .map(|kind| codewhale_config::ProviderChain::new(kind, &config.fallback_providers))
        .filter(|chain| chain.providers().len() > 1);
    app.last_fallback_reason = None;
    app.model_ids_passthrough = config.model_ids_pass_through();
    if !app.auto_model {
        let requested = app
            .reasoning_effort_preference
            .unwrap_or(app.reasoning_effort);
        app.reasoning_effort =
            requested.normalize_for_route(provider, &config.deepseek_base_url(), &app.model);
    }
    app.set_active_context_window_override(config.context_window_for_provider_config(provider));
    app.active_route_limits = app.context_window_override_limits();
    app.active_route_base_url = config.deepseek_base_url();
    app.active_context_window_source = if app.active_context_window_override.is_some() {
        crate::route_runtime::ContextWindowSource::Configured
    } else {
        crate::route_runtime::ContextWindowSource::Fallback
    };
}

pub(crate) fn resolve_loaded_session_route(app: &mut App, config: &Config) {
    let context_override = config.context_window_for_provider_config(app.api_provider);
    app.set_active_context_window_override(context_override);
    if app.auto_model {
        app.active_route_limits = app.context_window_override_limits();
        app.active_route_base_url = config.deepseek_base_url();
        app.active_context_window_source = if context_override.is_some() {
            crate::route_runtime::ContextWindowSource::Configured
        } else {
            crate::route_runtime::ContextWindowSource::Fallback
        };
        return;
    }

    let saved_provider_model = config
        .provider_config_for(app.api_provider)
        .and_then(|provider| provider.model.as_deref());
    match crate::route_runtime::resolve_route_candidate_with_context_metadata(
        app.api_provider,
        Some(&app.model),
        saved_provider_model,
        Some(config.deepseek_base_url()),
        context_override,
        None,
    ) {
        Ok(resolution) => {
            let resolved_model = resolution.candidate.wire_model_id().as_str().to_string();
            app.set_active_route_resolution(
                resolution.candidate.endpoint().base_url.clone(),
                resolution.candidate.limits(),
                resolution.context_window.source,
            );
            app.fleet_roster_stale |= crate::fleet::members::auto_enroll_fleet_model(
                &app.workspace,
                app.provider_identity_for_persistence(),
                &resolved_model,
            );
        }
        Err(_) => {
            app.active_route_limits = app.context_window_override_limits();
            app.active_route_base_url = config.deepseek_base_url();
            app.active_context_window_source = if context_override.is_some() {
                crate::route_runtime::ContextWindowSource::Configured
            } else {
                crate::route_runtime::ContextWindowSource::Fallback
            };
        }
    }
}

/// Derive a short display title from the API message list.
///
/// Tries several strategies in order:
/// 1. If the first user message starts with a known slash command (`/goal`,
///    `/fleet`, `/workflow`, etc.), use the command + first argument.
/// 2. Otherwise, take the first meaningful line and cut it at a natural
///    phrase boundary (period, comma, colon, or word boundary) within
///    `SESSION_TITLE_MAX_CHARS`, never splitting mid-word.
///
/// Never leaks raw prompt text — the result is always a concise label.
pub(crate) fn derive_session_title(messages: &[Message]) -> Option<String> {
    let text = messages.iter().find(|m| m.role == "user").and_then(|m| {
        m.content.iter().find_map(|block| match block {
            ContentBlock::Text { text, .. } if !text.starts_with(TURN_META_PREFIX) => {
                Some(text.trim().to_string())
            }
            _ => None,
        })
    })?;

    let first_line =
        crate::session_manager::sanitize_session_title(text.lines().next().unwrap_or("").trim());
    let first_line = first_line.trim();
    if first_line.is_empty() {
        return None;
    }

    // Slash command: extract command name + first reasonable argument.
    if let Some(rest) = first_line.strip_prefix('/') {
        let parts: Vec<&str> = rest.split_whitespace().collect();
        return match parts.as_slice() {
            [] => None,
            [cmd] => Some(format!("/{cmd}")),
            [cmd, arg, ..] => {
                let arg_short = short_title_truncate(arg, 24);
                Some(format!("/{cmd} {arg_short}"))
            }
        };
    }

    Some(short_title_truncate(first_line, SESSION_TITLE_MAX_CHARS))
}

#[cfg(test)]
mod derived_title_tests {
    use super::*;
    use crate::models::Role;

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }

    #[test]
    fn derived_titles_drop_terminal_controls_and_bidi_format_chars() {
        // The first user message can carry pasted escape sequences; the
        // derived session name must never persist them.
        let msgs = [user("Fix \u{1b}]0;PWNED\u{7}the\u{202e} build 会議")];
        assert_eq!(
            derive_session_title(&msgs).as_deref(),
            Some("Fix ]0;PWNEDthe build 会議")
        );
        // Controls alone leave no title to derive.
        assert_eq!(derive_session_title(&[user("\u{1b}\u{7}\u{200b}")]), None);
    }
}

#[cfg(test)]
mod stall_outbox_tests {
    use super::*;
    use crate::tui::app::TuiOptions;

    /// `recover_stalled_runtime_turn` must emit a `turn_stalled` lifecycle
    /// outbox event naming the wedged turn — the first scriptable stall
    /// signal. The outbox is opt-in, so the test enables it through config.
    #[tokio::test]
    async fn stalled_turn_emits_turn_stalled_outbox_event() {
        let _lock = crate::test_support::lock_test_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let outbox_path = dir.path().join("outbox.jsonl");

        let config = Config {
            lifecycle_outbox: Some(codewhale_config::LifecycleOutboxToml {
                path: Some(outbox_path.clone()),
                webhook_url: None,
                webhook_token: None,
            }),
            ..Default::default()
        };
        let options = TuiOptions {
            start_in_agent_mode: true,
            ..crate::test_support::test_tui_options(dir.path())
        };
        let mut app = App::new(options, &config);
        assert!(app.lifecycle_outbox.is_enabled());
        let expected_workspace = app.workspace.display().to_string();

        app.runtime_turn_id = Some("turn-1".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        app.is_loading = true;
        recover_stalled_runtime_turn(
            &mut app,
            "Turn stalled — no completion signal received",
            StatusToastLevel::Error,
        );

        // The outbox writer task drains asynchronously; wait for the line.
        let mut lines = Vec::new();
        for _ in 0..200 {
            if let Ok(text) = tokio::fs::read_to_string(&outbox_path).await {
                lines = text
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("json"))
                    .collect();
                if !lines.is_empty() {
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert_eq!(lines.len(), 1, "expected one turn_stalled outbox line");
        let line = &lines[0];
        assert_eq!(line["event"], "turn_stalled");
        assert_eq!(line["kind"], "turn.stalled");
        assert_eq!(line["turn_id"], "turn-1");
        assert_eq!(line["schema_version"], 1);
        assert_eq!(line["seq"], 1);
        // Every payload carries the workspace for consumer-side routing.
        assert_eq!(
            line["payload"]["workspace"],
            serde_json::json!(expected_workspace)
        );
        // The stall message is engine-authored and safe, but still bounded
        // and never raw tool/environment content.
        let message = line["payload"]["message"].as_str().expect("message");
        assert!(message.contains("stalled"));
        assert!(
            message.chars().count() <= codewhale_hooks::OUTBOX_DETAIL_MAX_CHARS,
            "stall message must be bounded"
        );
    }

    /// A disabled outbox (config without a path) must make stall recovery
    /// behave exactly as before: the toast still lands, no file is written.
    #[tokio::test]
    async fn stalled_turn_without_outbox_config_writes_nothing() {
        let _lock = crate::test_support::lock_test_env();
        let dir = tempfile::tempdir().expect("tempdir");
        let options = TuiOptions {
            start_in_agent_mode: true,
            ..crate::test_support::test_tui_options(dir.path())
        };
        let mut app = App::new(options, &Config::default());
        assert!(!app.lifecycle_outbox.is_enabled());

        app.runtime_turn_id = Some("turn-1".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        app.is_loading = true;
        recover_stalled_runtime_turn(
            &mut app,
            "Turn stalled — no completion signal received",
            StatusToastLevel::Error,
        );

        // Recovery still clears the wedged turn state and posts the toast.
        assert!(app.runtime_turn_id.is_none());
        assert!(!app.is_loading);
        assert!(!app.status_toasts.is_empty());
        assert!(
            !dir.path().join("outbox.jsonl").exists(),
            "no outbox file must be created when the feature is off"
        );
    }
}

#[cfg(test)]
mod launch_resume_tests {
    use super::*;

    /// A recent-work row that vanished behind the card must leave the card
    /// up with a status — never strand the user on an empty stage.
    #[test]
    fn resume_missing_session_leaves_the_card_up_with_a_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::test_support::test_tui_options(dir.path()),
            &Config::default(),
        );
        app.launch.visible = true;
        let result = resume_launch_session(&mut app, "no-such-session-000000");
        assert!(result.action.is_none(), "nothing to load");
        assert!(app.launch.visible, "the card stays up");
        let status = app.launch.status.as_deref().expect("a status");
        assert!(
            status.contains("Resume failed"),
            "the status says why: {status}"
        );
    }

    /// The prominent new-session entry begins a fresh session in place.
    #[test]
    fn new_session_begins_a_fresh_session_and_leaves_the_card() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = App::new(
            crate::test_support::test_tui_options(dir.path()),
            &Config::default(),
        );
        app.launch.visible = true;
        let result = begin_launch_session(&mut app, None);
        assert!(!app.launch.visible, "the session began");
        assert!(
            app.current_session_id.is_some(),
            "a fresh session id was minted"
        );
        assert!(
            matches!(result.action, Some(AppAction::SyncSession { .. })),
            "the engine syncs the fresh session"
        );
    }
}
