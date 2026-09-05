//! `handle_*` helpers: turning one input event, view event, or external
//! action into `App` state changes.
//!
//! Moved verbatim out of `ui.rs`.

use super::*;

/// How long the picker's ⇧F receipt stays in the footer: long enough to read
/// a route and its roles, short enough to leave the chrome still.
const FLEET_TOGGLE_TOAST_TTL_MS: u64 = 6_000;

/// Push the effective roster (saved fleet + config + plugins) to the running
/// engine through `Op::SetFleetRoster`. The one path every fleet mutation
/// takes: the saved-fleet views call it directly, and `/fleet add|remove`,
/// ⇧F, and auto-enroll reach it through `App::fleet_roster_stale`.
pub(crate) fn sync_fleet_roster(app: &mut App, config: &Config, engine_handle: &EngineHandle) {
    let roster = crate::fleet::identity::load_effective_roster(
        &config.fleet_config(),
        &app.workspace,
        Some(app.plugin_registry.as_ref()),
    );
    if let Some(error) = roster.load_error() {
        app.set_sticky_status(error.to_string(), StatusToastLevel::Error, None);
    }
    let _ = engine_handle.try_send(Op::SetFleetRoster {
        roster: std::sync::Arc::new(roster),
    });
}

/// Once per event-loop iteration: deliver a pending fleet mutation to the
/// engine and clear the flag.
pub(crate) fn flush_stale_fleet_roster(
    app: &mut App,
    config: &Config,
    engine_handle: &EngineHandle,
) {
    if std::mem::take(&mut app.fleet_roster_stale) {
        sync_fleet_roster(app, config, engine_handle);
    }
}

/// Persist a `# foo` quick-add through the native memory store and surface
/// a status note to the user. Errors land in the same status channel so a
/// missing memory directory becomes visible without crashing the composer.
pub(crate) fn handle_memory_quick_add(app: &mut App, input: &str, config: &Config) {
    let path = config.memory_path();
    let note = input.trim_start_matches('#').trim();
    let result = crate::native_memory::NativeMemoryStore::from_global_path(&path)
        .ok_or_else(|| format!("{} is not a native memory path", path.display()))
        .and_then(|store| {
            store
                .remember(crate::native_memory::MemoryScope::Global, None, note)
                .map(|hit| hit.source)
                .map_err(|err| err.to_string())
        });
    match result {
        Ok(source) => {
            app.status_message = Some(format!("memory: appended to {}", source.display()));
        }
        Err(err) => {
            app.status_message = Some(format!(
                "memory: failed to write {}: {}",
                path.display(),
                err
            ));
        }
    }
}

/// Route one terminal bracketed-paste event without exposing its contents.
///
/// Keeping the routing in one function makes the credential and ordinary
/// composer paths exercise the same observability boundary.
pub(crate) fn handle_bracketed_paste(app: &mut App, text: &str) {
    tracing::debug!(
        paste_bytes = text.len(),
        paste_chars = text.chars().count(),
        "Received bracketed paste event"
    );
    // Once a real bracketed-paste event has been observed in this session,
    // the rapid-keystroke heuristic in paste_burst is redundant — disable it
    // so fast typing / IME commits / autocomplete bursts don't get
    // mis-classified as a paste.
    app.bracketed_paste_seen = true;
    if app.is_history_search_active() {
        app.history_search_insert_str(text);
    } else if paste_text_into_provider_picker(app, text) || app.view_stack.handle_paste(text) {
        // Modal consumed the paste (e.g. provider picker key entry).
    } else if !app.view_stack.is_empty() {
        // A non-consumed modal is open — don't leak paste into composer.
    } else {
        // Paste into main input.
        app.insert_paste_text(text);
    }
}

/// Voice input toggle via Option+V (⌥V) — matches Muse Spark UX:
/// "Recording (⌥V to finish)" with a transient voice indicator, no slash
/// command needed. Handles both Alt+V and the macOS ⌥V glyph.
pub(crate) fn handle_voice_key(app: &mut App, key: &event::KeyEvent) -> bool {
    let is_alt_v = matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
        && key.modifiers.contains(KeyModifiers::ALT)
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::SUPER);
    // Some terminals emit the literal "√" (Option+V on macOS) instead of Alt+V.
    let is_glyph = matches!(key.code, KeyCode::Char('√') | KeyCode::Char('∫'));
    if !is_alt_v && !is_glyph {
        return false;
    }
    // Toggle voice capture — same path as /voice but via hotkey.
    let result = crate::commands::voice::voice(app);
    // Surface a Spark-style transient hint; the capture itself is async.
    if app.voice_enabled {
        app.status_message = Some("● Recording  (⌥V to finish)".to_string());
    }
    // Suppress the default char insertion for this combo.
    let _ = result;
    true
}

/// The event-loop seam for Ctrl+T. Keeping the `KeyEvent` predicate and App
/// mutation together makes the real terminal route directly testable rather
/// than testing `cycle_effort` in isolation.
pub(crate) fn handle_reasoning_effort_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if !matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T'))
        || key.modifiers != KeyModifiers::CONTROL
    {
        return false;
    }
    let _ = app.cycle_effort();
    true
}

/// Let the transcript remain reviewable while an approval card owns focus.
pub(crate) fn handle_approval_transcript_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if app.view_stack.top_kind() != Some(ModalKind::Approval) {
        return false;
    }

    let page = app.viewport.last_transcript_visible.max(1);
    match key.code {
        KeyCode::PageUp => app.scroll_up(page),
        KeyCode::PageDown => app.scroll_down(page),
        KeyCode::Up
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.scroll_up(3);
        }
        KeyCode::Down
            if key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT | KeyModifiers::CONTROL) =>
        {
            app.scroll_down(3);
        }
        KeyCode::Home => app.scroll_up(usize::MAX),
        KeyCode::End => app.scroll_to_bottom(),
        _ => return false,
    }
    true
}

/// Route only non-text controls to a focused workflow panel.
///
/// Returning `false` for every character is deliberate: the caller then lets
/// the normal composer path insert it. A prior bare-letter contract used
/// t/c/j/k here, which made the first matching letter of a new chat disappear
/// after the user clicked the workflow card.
pub(crate) fn handle_workflow_panel_key(app: &mut App, key: &event::KeyEvent) -> bool {
    if !app
        .workflow_panel
        .as_ref()
        .is_some_and(|panel| panel.keyboard_focus)
    {
        return false;
    }

    if matches!(key.code, KeyCode::Char(_)) {
        if let Some(panel) = app.workflow_panel.as_mut() {
            panel.keyboard_focus = false;
        }
        app.needs_redraw = true;
        return false;
    }

    if !key.modifiers.is_empty() && key.code != KeyCode::Esc {
        return false;
    }

    match key.code {
        KeyCode::Esc => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.keyboard_focus = false;
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Enter => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                let _ = panel.toggle_expanded();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Down => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.select_next_phase();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Up => {
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.select_prev_phase();
            }
            app.needs_redraw = true;
            true
        }
        KeyCode::Delete => {
            let Some(run_id) = app
                .workflow_panel
                .as_ref()
                .and_then(|panel| panel.lifecycle.is_running().then(|| panel.run_id.clone()))
            else {
                return false;
            };
            app.input = format!("/workflow cancel {run_id}");
            app.cursor_position = app.input.chars().count();
            app.status_message = Some(app.tr(MessageId::SidebarDestructiveArmed).into_owned());
            if let Some(panel) = app.workflow_panel.as_mut() {
                panel.keyboard_focus = false;
            }
            app.needs_redraw = true;
            true
        }
        _ => false,
    }
}

/// One-shot "draft my constitution" call against the user's first configured
/// model, requested by `A` on the setup Constitution card. Runs inline in the
/// event loop like [`fetch_available_models`] (the wizard modal stays open
/// underneath) with a hard timeout so a slow provider cannot wedge setup.
///
/// On success the sanitized, bounded draft is installed into the open wizard
/// and its ratification preview opens on top — nothing persists until the
/// user ratifies with `G`. Every failure (no client, timeout, request error,
/// invalid or empty JSON) is a status line, never an error state: the
/// deterministic guided draft remains the standing fallback.
pub(crate) async fn handle_setup_constitution_model_draft(
    app: &mut App,
    config: &Config,
    draft: crate::tui::setup::GuidedConstitutionDraft,
    freeform_note: Option<String>,
    locale: crate::localization::Locale,
) {
    // Spawn the draft off the event loop (same pattern as the fleet drafter,
    // #3757 review): awaiting it inline parked the whole TUI for up to the
    // timeout. The loop polls constitution_draft_cell and delivers the result.
    const DRAFT_TIMEOUT: Duration = Duration::from_secs(20);
    let model_label = app.model_display_label();
    let client = match DeepSeekClient::new(config) {
        Ok(client) => client,
        Err(err) => {
            deliver_constitution_draft_result(
                app,
                model_label.clone(),
                locale,
                Err(format!("provider not ready: {err:#}")),
            );
            return;
        }
    };
    let request_model = app.model.clone();
    let cell = app.constitution_draft_cell.clone();
    let spawn_label = model_label.clone();
    let request_gen = app.next_draft_gen();
    app.status_message = Some(match locale {
        crate::localization::Locale::ZhHans => {
            format!(
                "{model_label} 正在生成协作准则草案……（最多 {}s）",
                DRAFT_TIMEOUT.as_secs()
            )
        }
        _ => format!(
            "{model_label} is drafting your constitution… (up to {}s)",
            DRAFT_TIMEOUT.as_secs()
        ),
    });
    app.needs_redraw = true;
    tokio::spawn(async move {
        let outcome = match tokio::time::timeout(
            DRAFT_TIMEOUT,
            crate::tui::setup::draft_constitution_with_model(
                &client,
                &request_model,
                draft,
                freeform_note,
                locale,
            ),
        )
        .await
        {
            Err(_) => Err(format!("timed out after {}s", DRAFT_TIMEOUT.as_secs())),
            Ok(result) => result,
        };
        if let Ok(mut guard) = cell.lock() {
            *guard = Some((request_gen, spawn_label, locale, outcome));
        }
    });
}

/// One-shot fleet-profile draft: same contract as the constitution drafter —
/// minimal payload out, untrusted gate in, preview before ratify, degrade to
/// the manual authoring flow on any failure.
pub(crate) async fn handle_fleet_profile_model_draft(
    app: &mut App,
    config: &Config,
    role: String,
    model: String,
    provider: Option<String>,
    reasoning_effort: Option<String>,
    locale: crate::localization::Locale,
) {
    // The route the operator actually picked at `m`-press time (#4093). A
    // model draft always comes back `provider: None` (the untrusted gate
    // strips any provider), so this captured `(provider, model)` is what the
    // ratified profile is pinned to — immune to the model omitting/altering
    // the route AND to the selection changing while the draft is in flight.
    // `None` for an `inherit` pick (no concrete route to keep).
    let picked_route = provider.map(|provider| (provider, model.clone()));
    // Do NOT await the network call on the event loop — that parks the whole
    // TUI for up to the timeout (#3757 review). Spawn it into the shared
    // fleet_draft_cell and let the loop poll + deliver the result, keeping
    // the wizard interactive with a drafting status.
    const DRAFT_TIMEOUT: Duration = Duration::from_secs(20);
    let model_label = app.model_display_label();
    let client = match DeepSeekClient::new(config) {
        Ok(client) => client,
        Err(err) => {
            deliver_fleet_draft_result(
                app,
                model_label.clone(),
                picked_route.clone(),
                reasoning_effort.clone(),
                Err(format!("provider not ready: {err:#}")),
                locale,
            );
            return;
        }
    };
    let request_model = app.model.clone();
    let cell = app.fleet_draft_cell.clone();
    let spawn_label = model_label.clone();
    let request_gen = app.next_draft_gen();
    let workspace = app.workspace.clone();
    app.status_message = Some(match locale {
        crate::localization::Locale::ZhHans => {
            format!(
                "{model_label} 正在起草配置……（最多 {}s）",
                DRAFT_TIMEOUT.as_secs()
            )
        }
        _ => format!(
            "{model_label} is drafting the profile… (up to {}s)",
            DRAFT_TIMEOUT.as_secs()
        ),
    });
    app.needs_redraw = true;
    tokio::spawn(async move {
        // Redacted, bounded workspace fingerprint (manifest names, test
        // commands, branch + dirty count — no contents, secrets, or absolute
        // paths). Computed off the event loop; the untrusted-output gate on
        // the reply is unchanged.
        let fingerprint = tokio::task::spawn_blocking(move || {
            crate::tui::setup::workspace_fingerprint(&workspace)
        })
        .await
        .unwrap_or_default();
        let outcome = match tokio::time::timeout(
            DRAFT_TIMEOUT,
            crate::tui::setup::draft_fleet_profile_with_model(
                &client,
                &request_model,
                &role,
                &model,
                locale,
                &fingerprint,
            ),
        )
        .await
        {
            Err(_) => Err(format!("timed out after {}s", DRAFT_TIMEOUT.as_secs())),
            Ok(result) => result,
        };
        if let Ok(mut guard) = cell.lock() {
            *guard = Some((
                request_gen,
                spawn_label,
                picked_route,
                reasoning_effort,
                outcome,
            ));
        }
    });
}

pub(crate) async fn handle_bang_shell_input(
    app: &mut App,
    engine_handle: &EngineHandle,
    input: &str,
) -> Result<bool> {
    let command = match shell_command_from_bang_input(input) {
        Ok(Some(command)) => command,
        Ok(None) => return Ok(false),
        Err(message) => {
            app.status_message = Some(format!("Error: {message}"));
            return Ok(true);
        }
    };

    engine_handle
        .send(Op::RunShellCommand {
            command: command.to_string(),
            mode: app.mode,
            allow_shell: app.allow_shell,
            trust_mode: app.trust_mode,
            auto_approve: app_auto_approve_enabled(app),
            approval_mode: app.approval_mode,
        })
        .await?;
    app.status_message = Some(format!("Shell command submitted: {command}"));
    Ok(true)
}

pub(crate) async fn handle_mcp_ui_action(
    app: &mut App,
    engine_handle: &EngineHandle,
    config: &Config,
    action: crate::tui::app::McpUiAction,
) {
    use crate::mcp::{self, McpWriteStatus};

    let path = app.mcp_config_path.clone();
    let mut changed = false;
    let mut message = None;
    let is_reload = matches!(&action, crate::tui::app::McpUiAction::Reload);
    let retry_name = match &action {
        crate::tui::app::McpUiAction::Retry { name } => Some(name.clone()),
        _ => None,
    };
    let snapshot_live_pool = matches!(&action, crate::tui::app::McpUiAction::Show);
    let discover = mcp_ui_action_refreshes_discovery(&action);

    let action_result = match action {
        crate::tui::app::McpUiAction::Show => Ok(()),
        crate::tui::app::McpUiAction::Init { force } => {
            changed = true;
            match mcp::init_config(&path, force) {
                Ok(McpWriteStatus::Created) => {
                    message = Some(format!("Created MCP config at {}", path.display()));
                    Ok(())
                }
                Ok(McpWriteStatus::Overwritten) => {
                    message = Some(format!("Overwrote MCP config at {}", path.display()));
                    Ok(())
                }
                Ok(McpWriteStatus::SkippedExists) => {
                    changed = false;
                    message = Some(format!(
                        "MCP config already exists at {} (use /mcp init --force to overwrite)",
                        path.display()
                    ));
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        crate::tui::app::McpUiAction::AddStdio {
            name,
            command,
            args,
        } => {
            changed = true;
            mcp::add_server_config(&path, name.clone(), Some(command), None, args, None)
                .map(|()| message = Some(format!("Added MCP stdio server '{name}'")))
        }
        crate::tui::app::McpUiAction::AddHttp {
            name,
            url,
            transport,
        } => {
            changed = true;
            mcp::add_server_config(&path, name.clone(), None, Some(url), Vec::new(), transport)
                .map(|()| message = Some(format!("Added MCP HTTP/SSE server '{name}'")))
        }
        crate::tui::app::McpUiAction::Enable { name } => {
            changed = true;
            mcp::set_server_enabled(&path, &name, true)
                .map(|()| message = Some(format!("Enabled MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Disable { name } => {
            changed = true;
            mcp::set_server_enabled(&path, &name, false)
                .map(|()| message = Some(format!("Disabled MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Remove { name } => {
            changed = true;
            mcp::remove_server_config(&path, &name)
                .map(|()| message = Some(format!("Removed MCP server '{name}'")))
        }
        crate::tui::app::McpUiAction::Login { name, scopes } => {
            let result = async {
                let cfg = mcp::load_config_with_workspace_and_plugins(
                    &path,
                    &app.workspace,
                    app.plugin_registry.as_ref(),
                )?;
                let server = cfg
                    .servers
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
                let explicit_scopes = (!scopes.is_empty()).then_some(scopes);
                mcp::oauth::perform_oauth_login_for_server(
                    &name,
                    server,
                    explicit_scopes,
                    config.mcp_oauth_callback_port,
                    config.mcp_oauth_callback_url.as_deref(),
                )
                .await
            }
            .await;
            result.map(|()| {
                changed = true;
                message = Some(format!(
                    "Stored OAuth credentials for MCP server '{name}'. Run /mcp reload to reconnect it."
                ));
            })
        }
        crate::tui::app::McpUiAction::Logout { name } => {
            let result = (|| {
                let cfg = mcp::load_config_with_workspace_and_plugins(
                    &path,
                    &app.workspace,
                    app.plugin_registry.as_ref(),
                )?;
                let server = cfg
                    .servers
                    .get(&name)
                    .ok_or_else(|| anyhow::anyhow!("MCP server '{name}' not found"))?;
                mcp::oauth::delete_oauth_tokens_for_server(&name, server)
            })();
            result.map(|deleted| {
                changed = deleted;
                message = Some(if deleted {
                    format!(
                        "Deleted stored OAuth credentials for MCP server '{name}'. Run /mcp reload to reconnect it."
                    )
                } else {
                    format!("No stored OAuth credentials found for MCP server '{name}'.")
                });
            })
        }
        crate::tui::app::McpUiAction::ImportList => {
            let text = mcp_external_import_status_text(&app.workspace);
            message = Some(text);
            Ok(())
        }
        crate::tui::app::McpUiAction::ImportApprove { name } => {
            match mcp_import_apply(&app.workspace, &path, &name, true) {
                Ok(msg) => {
                    changed = msg.contains("Imported");
                    message = Some(msg);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        crate::tui::app::McpUiAction::ImportDecline { name } => {
            match mcp_import_apply(&app.workspace, &path, &name, false) {
                Ok(msg) => {
                    message = Some(msg);
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        crate::tui::app::McpUiAction::Validate | crate::tui::app::McpUiAction::Reload => Ok(()),
        crate::tui::app::McpUiAction::Retry { .. } => Ok(()),
    };

    if let Err(err) = action_result {
        add_mcp_message(app, format!("MCP action failed: {err}"));
        return;
    }

    if changed {
        app.mcp_reload_required = true;
    }
    if let Some(message) = message {
        add_mcp_message(app, message);
    }

    // A successful MCP mutation is an explicit request to change the tools
    // available to this running session. Apply it to the engine-owned pool in
    // the same operation instead of leaving Extensions and `/mcp` users on a
    // second, easy-to-miss reload step. The standalone reload action remains
    // the retry/compatibility path for externally edited configuration.
    let rebuild_live_pool = is_reload || changed;
    let snapshot_result = if let Some(name) = retry_name.as_deref() {
        engine_handle
            .retry_mcp_server(name)
            .await
            .map(|update| (update.snapshot, Some(update.generation)))
    } else if snapshot_live_pool {
        engine_handle
            .bootstrap_mcp()
            .await
            .map(|update| (update.snapshot, Some(update.generation)))
    } else if rebuild_live_pool {
        match engine_handle.reload_mcp(path.clone()).await {
            Ok(update) => {
                app.mcp_reload_required = false;
                add_mcp_message(app, mcp_reload_summary(&update.snapshot));
                Ok((update.snapshot, Some(update.generation)))
            }
            Err(error) => {
                app.mcp_reload_required = true;
                Err(error)
            }
        }
    } else if discover {
        let network_policy = config.network.clone().map(|toml_cfg| {
            crate::network_policy::NetworkPolicyDecider::with_default_audit(toml_cfg.into_runtime())
        });
        mcp::discover_manager_snapshot_with_workspace_and_plugins(
            &path,
            &app.workspace,
            network_policy,
            app.mcp_reload_required,
            std::sync::Arc::clone(&app.plugin_registry),
        )
        .await
        .map(|snapshot| (snapshot, None))
    } else {
        mcp::manager_snapshot_from_config_with_workspace_and_plugins(
            &path,
            &app.workspace,
            app.mcp_reload_required,
            app.plugin_registry.as_ref(),
        )
        .map(|snapshot| (snapshot, None))
    };

    match snapshot_result {
        Ok((snapshot, generation)) => {
            if discover {
                add_mcp_message(
                    app,
                    "MCP discovery refreshed for the UI. Run /mcp reload after config or credential edits to rebuild the live model-visible tool pool.".to_string(),
                );
            }
            // Keep the boot-time MCP-count chip in sync with the live
            // snapshot so footers and panels reflect post-/mcp edits
            // (#502).
            app.mcp_configured_count = snapshot.servers.len();
            if let Some(generation) = generation {
                app.mcp_snapshot_generation = generation;
                app.mcp_snapshot_generation_invalidated = true;
            }
            app.mcp_snapshot = Some(snapshot.clone());
            app.mcp_initializing = false;
            app.mcp_connecting.clear();
            // #2068: keep the hotbar's MCP-tool actions in sync with the tools
            // that are actually loaded; the hotbar never connects on its own.
            app.hotbar_actions.replace_mcp_tools(Some(&snapshot));
            open_mcp_manager_pager(app, &snapshot);
        }
        Err(err) if retry_name.is_some() => add_mcp_message(
            app,
            format!("MCP server retry failed; the live tool pool is unchanged: {err}"),
        ),
        Err(err) if rebuild_live_pool => add_mcp_message(
            app,
            format!("MCP reload failed; the live tool pool is unchanged: {err}"),
        ),
        Err(err) => add_mcp_message(app, format!("MCP snapshot failed: {err}")),
    }
}

pub(crate) fn handle_shell_job_action(app: &mut App, action: crate::tui::app::ShellJobAction) {
    let Some(shell_manager) = app.runtime_services.shell_manager.clone() else {
        add_shell_job_message(app, "No shell session is active.".to_string());
        return;
    };

    let mut manager = match shell_manager.lock() {
        Ok(manager) => manager,
        Err(_) => {
            add_shell_job_message(
                app,
                "Shell tracking hit an internal error — restart Codewhale to recover.".to_string(),
            );
            return;
        }
    };
    let active_session_id = app.current_session_id.clone().unwrap_or_default();

    match action {
        crate::tui::app::ShellJobAction::List => {
            let jobs = manager.list_jobs_for_session(&active_session_id);
            let mut text = format_shell_job_list(&jobs);
            if let Ok(cloud) =
                crate::cloud_dispatch::CloudJobStore::from_env().and_then(|store| store.list())
                && !cloud.is_empty()
            {
                text.push_str("\n\n");
                text.push_str(&crate::cloud_dispatch::format_job_list(&cloud));
            }
            add_shell_job_message(app, text);
        }
        crate::tui::app::ShellJobAction::Show { id } => {
            match manager.inspect_job_for_session(&active_session_id, &id) {
                Ok(detail) => open_shell_job_pager(app, &detail),
                Err(err) => add_shell_job_message(app, format!("Command lookup failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::Poll { id, wait } => {
            match manager.poll_delta_for_session(
                &active_session_id,
                &id,
                wait,
                if wait { 5_000 } else { 1_000 },
            ) {
                Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                Err(err) => add_shell_job_message(app, format!("Command poll failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::SendStdin { id, input, close } => {
            match manager.write_stdin_for_session(&active_session_id, &id, &input, close) {
                Ok(()) => {
                    match manager.poll_delta_for_session(&active_session_id, &id, false, 1_000) {
                        Ok(delta) => add_shell_job_message(app, format_shell_poll(&delta.result)),
                        Err(err) => {
                            add_shell_job_message(
                                app,
                                format!("Command input sent; poll failed: {err}"),
                            );
                        }
                    }
                }
                Err(err) => add_shell_job_message(app, format!("Command input failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::Cancel { id } => {
            match manager.kill_for_session(&active_session_id, &id) {
                Ok(result) => add_shell_job_message(app, format_shell_poll(&result)),
                Err(err) => add_shell_job_message(app, format!("Command cancel failed: {err}")),
            }
        }
        crate::tui::app::ShellJobAction::CancelAll => {
            match manager.kill_running_for_session(&active_session_id) {
                Ok(results) => {
                    let count = results.len();
                    if count == 0 {
                        add_shell_job_message(app, "No running commands to cancel.".to_string());
                    } else {
                        let tasks: Vec<String> = results
                            .iter()
                            .filter_map(|result| result.task_id.clone())
                            .collect();
                        add_shell_job_message(
                            app,
                            format!("Canceled {count} command(s): {}", tasks.join(", ")),
                        );
                    }
                }
                Err(err) => add_shell_job_message(app, format!("Command cancel-all failed: {err}")),
            }
        }
    }
}

pub(crate) async fn handle_skill_mutation_requested(
    app: &mut App,
    request: crate::skills::mutation::SkillMutationRequest,
) {
    use crate::skills::install::{DEFAULT_MAX_SIZE_BYTES, DEFAULT_REGISTRY_URL};
    use crate::skills::mutation::{MutationContext, SkillMutationOutcome, SkillMutationRequest};

    let focus = match &request {
        SkillMutationRequest::ImportExternal { source_id, .. } => Some(source_id.clone()),
        SkillMutationRequest::Update { skill_id, .. }
        | SkillMutationRequest::Remove { skill_id, .. }
        | SkillMutationRequest::Trust { skill_id, .. } => Some(skill_id.clone()),
        SkillMutationRequest::InstallRemote { .. }
        | SkillMutationRequest::UpdateByName { .. }
        | SkillMutationRequest::RemoveByName { .. }
        | SkillMutationRequest::TrustByName { .. } => None,
    };

    let workspace = app.workspace.clone();
    let home = crate::config::effective_home_dir();
    let cfg = crate::config::Config::load(None, None).unwrap_or_default();
    let network = cfg
        .network
        .clone()
        .map(|policy| policy.into_runtime())
        .unwrap_or_default();
    let skills_cfg = cfg.skills.as_ref();
    let max_size = skills_cfg
        .and_then(|s| s.max_install_size_bytes)
        .unwrap_or(DEFAULT_MAX_SIZE_BYTES);
    let registry_url = skills_cfg
        .and_then(|s| s.registry_url.clone())
        .unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    let skills_dir = app.skills_dir.clone();
    let result = {
        let ctx = MutationContext {
            workspace: &workspace,
            home: home.as_deref(),
            configured_skills_dir: Some(skills_dir.as_path()),
            network: &network,
            max_size,
            registry_url: &registry_url,
        };
        crate::skills::mutation::execute(request, &ctx).await
    };

    let (status, refresh_skills) = match result {
        Ok(receipt) => {
            let msg = match &receipt.outcome {
                SkillMutationOutcome::Installed => {
                    format!(
                        "Installed '{}' → {}",
                        receipt.name, receipt.safe_target_path
                    )
                }
                SkillMutationOutcome::Updated => format!("Updated '{}'", receipt.name),
                SkillMutationOutcome::NoChange => {
                    format!("'{}': no upstream change", receipt.name)
                }
                SkillMutationOutcome::Removed => format!("Removed '{}'", receipt.name),
                SkillMutationOutcome::Trusted => format!("Trusted '{}'", receipt.name),
                SkillMutationOutcome::Imported => {
                    format!("Imported '{}' → {}", receipt.name, receipt.safe_target_path)
                }
                SkillMutationOutcome::AlreadyPresent => {
                    format!("'{}' already present (exact duplicate)", receipt.name)
                }
                SkillMutationOutcome::NeedsApproval(host) => {
                    format!("Needs network approval for {host}")
                }
                SkillMutationOutcome::NetworkDenied(host) => {
                    format!("Network denied for {host}")
                }
            };
            let refresh = !matches!(
                receipt.outcome,
                SkillMutationOutcome::NeedsApproval(_) | SkillMutationOutcome::NetworkDenied(_)
            );
            (msg, refresh)
        }
        Err(err) => (format!("Skill mutation failed: {err:#}"), false),
    };

    app.status_message = Some(status.clone());
    if refresh_skills {
        app.refresh_skill_cache();
    }
    refresh_skills_manager_if_open(app, Some(status), focus.as_ref());
    app.needs_redraw = true;
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_config_updated(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    task_manager: &SharedTaskManager,
    engine_handle: &mut EngineHandle,
    web_config_session: &mut Option<WebConfigSession>,
    key: String,
    value: String,
    persist: bool,
) -> Result<bool> {
    let result = prepare_config_update_result(
        commands::set_config_value(app, &key, &value, persist),
        persist,
    );
    let telemetry_toast = (key == "telemetry")
        .then(|| {
            result.message.clone().map(|message| {
                let level = if result.is_error {
                    StatusToastLevel::Error
                } else {
                    StatusToastLevel::Success
                };
                (message, level)
            })
        })
        .flatten();
    let normalized_value = value.trim().to_ascii_lowercase().replace([' ', '_'], "-");
    let cleared_root_approval = !result.is_error
        && persist
        && key == "approval_policy"
        && matches!(
            normalized_value.as_str(),
            "default" | "tui-default" | "use-tui-default"
        );
    // Theme / background changes require a full terminal repaint because
    // ratatui's incremental diff cannot see colors remapped by the backend.
    if matches!(
        key.as_str(),
        "theme" | "ui_theme" | "background_color" | "background" | "bg"
    ) {
        app.force_next_full_repaint = true;
    }
    if apply_command_result(
        terminal,
        app,
        engine_handle,
        task_manager,
        config,
        web_config_session,
        result,
    )
    .await?
    {
        return Ok(true);
    }

    let focus_key = if cleared_root_approval {
        "permission_posture"
    } else {
        &key
    };
    refresh_config_view_if_open(app, focus_key);
    if let Some((message, level)) = telemetry_toast {
        // The modal stays open, so a transcript-only command receipt would be
        // invisible. Keep the durable disk truth in the rebuilt row and show
        // the localized result above it.
        app.push_status_toast(message, level, Some(12_000));
    }
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
async fn handle_theme_selection_updated(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    task_manager: &SharedTaskManager,
    engine_handle: &mut EngineHandle,
    web_config_session: &mut Option<WebConfigSession>,
    theme: String,
    persist: bool,
) -> Result<bool> {
    let result = prepare_config_update_result(
        commands::set_config_value(app, "theme", &theme, persist),
        persist,
    );
    // The theme owns the shell paint and must bypass ratatui's incremental
    // cell diff, including an Esc rollback.
    app.force_next_full_repaint = true;
    if apply_command_result(
        terminal,
        app,
        engine_handle,
        task_manager,
        config,
        web_config_session,
        result,
    )
    .await?
    {
        return Ok(true);
    }
    refresh_config_view_if_open(app, "theme");
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_view_events(
    terminal: &mut AppTerminal,
    app: &mut App,
    config: &mut Config,
    task_manager: &SharedTaskManager,
    engine_handle: &mut EngineHandle,
    web_config_session: &mut Option<WebConfigSession>,
    events: Vec<ViewEvent>,
) -> Result<bool> {
    for event in events {
        match event {
            ViewEvent::CommandPaletteSelected { action } => match action {
                crate::tui::views::CommandPaletteAction::ExecuteCommand { command } => {
                    if execute_command_input(
                        terminal,
                        app,
                        engine_handle,
                        task_manager,
                        config,
                        &mut *web_config_session,
                        &command,
                    )
                    .await?
                    {
                        return Ok(true);
                    }
                }
                crate::tui::views::CommandPaletteAction::InsertText { text } => {
                    app.input = text;
                    app.cursor_position = app.input.chars().count();
                    app.status_message = Some(
                        "Inserted into composer. Finish the input or press Enter.".to_string(),
                    );
                }
                crate::tui::views::CommandPaletteAction::OpenTextPager { title, content } => {
                    open_text_pager(app, title, content);
                }
            },
            ViewEvent::OpenTextPager { title, content } => {
                open_text_pager(app, title, content);
            }
            ViewEvent::CopyToClipboard { text, label } => {
                if text.is_empty() {
                    app.status_message = Some(format!("{label} is empty"));
                } else if app.clipboard.write_text(&text).is_ok() {
                    app.status_message = Some(format!("{label} copied"));
                } else {
                    app.status_message = Some(format!("Copy failed ({label})"));
                }
            }
            ViewEvent::ApprovalDecision {
                tool_id,
                tool_name,
                decision,
                timed_out,
                approval_key,
                approval_grouping_key,
                persistent_rules,
            } => {
                apply_approval_decision(
                    app,
                    engine_handle,
                    config,
                    ApprovalDecisionEvent {
                        tool_id,
                        tool_name,
                        decision,
                        timed_out,
                        approval_key,
                        approval_grouping_key,
                        persistent_rules,
                    },
                )
                .await;

                if timed_out {
                    app.add_message(HistoryCell::System {
                        content: "Approval request timed out - denied".to_string(),
                    });
                }
            }
            ViewEvent::ElevationDecision {
                tool_id,
                tool_name,
                option,
            } => {
                use crate::tui::approval::ElevationOption;
                match option {
                    ElevationOption::Abort => {
                        let _ = engine_handle.deny_tool_call(tool_id).await;
                        app.add_message(HistoryCell::System {
                            content: format!("Sandbox elevation aborted for {tool_name}"),
                        });
                    }
                    ElevationOption::WithNetwork => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with network access enabled"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                    ElevationOption::WithWriteAccess(_) => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with write access enabled"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                    ElevationOption::FullAccess => {
                        app.add_message(HistoryCell::System {
                            content: format!("Retrying {tool_name} with full access (no sandbox)"),
                        });
                        let policy = option.to_policy(&app.workspace);
                        let _ = engine_handle.retry_tool_with_policy(tool_id, policy).await;
                    }
                }
            }
            ViewEvent::UserInputSubmitted { tool_id, response } => {
                match engine_handle
                    .submit_user_input(tool_id.clone(), response)
                    .await
                {
                    Ok(()) => {
                        app.pending_user_input_prompt = None;
                    }
                    Err(err) => {
                        tracing::warn!(tool_id = %tool_id, error = %err, "user input submit failed");
                        if let Some((id, request)) = app.pending_user_input_prompt.clone() {
                            app.view_stack.push(UserInputView::new(id, request));
                        }
                        app.push_status_toast(
                            format!("Failed to submit response: {err}"),
                            StatusToastLevel::Error,
                            None,
                        );
                        app.status_message =
                            Some(format!("Failed to submit response: {err} — try again"));
                    }
                }
            }
            ViewEvent::UserInputCancelled { tool_id } => {
                let _ = engine_handle.cancel_user_input(tool_id).await;
                app.add_message(HistoryCell::System {
                    content: "User input cancelled".to_string(),
                });
            }
            ViewEvent::SessionSelected { session_id } => {
                let manager = match SessionManager::default_location() {
                    Ok(manager) => manager,
                    Err(err) => {
                        app.status_message =
                            Some(format!("Failed to open sessions directory: {err}"));
                        continue;
                    }
                };

                match manager.load_session(&session_id) {
                    Ok(session) => {
                        let next_config = config.clone();
                        let respawn = match apply_loaded_session_config_snapshot(
                            app,
                            config,
                            &session,
                            next_config,
                            false,
                        ) {
                            Ok(outcome) => outcome,
                            Err(err) => {
                                app.status_message =
                                    Some(format!("Failed to restore session: {err}"));
                                continue;
                            }
                        };
                        sync_runtime_workspace_state(task_manager, app.workspace.clone()).await;
                        if respawn {
                            let _ = engine_handle.send(Op::Shutdown).await;
                            *engine_handle =
                                spawn_tui_engine(build_engine_config(app, config), config);
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
                        // Durable receipt, matching `/load`: the status toast
                        // alone is replaced by the next footer update, leaving
                        // no findable record that the resume happened.
                        let loaded_message = format!(
                            "Session loaded (ID: {}, {} messages)",
                            crate::session_manager::truncate_id(&session_id),
                            session.metadata.message_count
                        );
                        app.add_message(HistoryCell::System {
                            content: loaded_message.clone(),
                        });
                        app.status_message = Some(loaded_message);
                        app.launch.visible = false;
                        app.launch.status = None;
                    }
                    Err(err) => {
                        app.status_message = Some(format!(
                            "Failed to load session {}: {err}",
                            crate::session_manager::truncate_id(&session_id)
                        ));
                    }
                }
            }
            ViewEvent::SessionRenamed { metadata } => {
                let session_id = metadata.id.clone();
                let title = metadata.title.clone();
                let mut work_snapshot_warning = None;
                if apply_picker_session_rename_to_active_app(app, *metadata)
                    && let Ok(manager) = SessionManager::default_location()
                {
                    match build_session_snapshot(app, &manager) {
                        Ok(session) => {
                            if let Err(err) = persist_with_pending_work_boundary(
                                app,
                                PersistRequest::SessionSnapshot(session),
                            ) {
                                tracing::warn!(
                                    session_id = %session_id,
                                    error = %err,
                                    "Could not queue active session rename Work snapshot"
                                );
                                work_snapshot_warning = Some(format!(
                                    "Session renamed, but Work snapshot is pending ({err})"
                                ));
                            }
                        }
                        Err(err) => {
                            tracing::warn!(
                                session_id = %session_id,
                                error = %err,
                                "Could not queue active session rename snapshot"
                            );
                        }
                    }
                }
                app.status_message = Some(work_snapshot_warning.unwrap_or_else(|| {
                    format!(
                        "Renamed session {} to \"{}\"",
                        crate::session_manager::truncate_id(&session_id),
                        title
                    )
                }));
            }
            ViewEvent::SessionArchived { metadata } => {
                // The manager already wrote the flag. Keep the active app's
                // cached metadata in step so the next autosave carries the new
                // state forward instead of reverting it, and drop the rail
                // cache so the row disappears (or returns) immediately.
                if let Some(cached) = app.current_session_metadata.as_mut()
                    && cached.id == metadata.id
                {
                    cached.archived = metadata.archived;
                }
                app.status_message = Some(format!(
                    "{} session {} ({})",
                    if metadata.archived {
                        "Archived"
                    } else {
                        "Restored"
                    },
                    crate::session_manager::truncate_id(&metadata.id),
                    metadata.title
                ));
            }
            ViewEvent::SessionDeleted { session_id, title } => {
                app.status_message = Some(format!(
                    "Deleted session {} ({})",
                    crate::session_manager::truncate_id(&session_id),
                    title
                ));
            }
            ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            } => {
                if handle_config_updated(
                    terminal,
                    app,
                    config,
                    task_manager,
                    engine_handle,
                    web_config_session,
                    key,
                    value,
                    persist,
                )
                .await?
                {
                    return Ok(true);
                }
            }
            ViewEvent::ThemeSelectionUpdated { theme, persist } => {
                if handle_theme_selection_updated(
                    terminal,
                    app,
                    config,
                    task_manager,
                    engine_handle,
                    web_config_session,
                    theme,
                    persist,
                )
                .await?
                {
                    return Ok(true);
                }
            }
            ViewEvent::StatusItemsUpdated { items, final_save } => {
                // Apply to the live App immediately so the footer reflects
                // every keystroke (live preview).
                app.status_items = items.clone();
                app.needs_redraw = true;
                if final_save {
                    match crate::config_persistence::persist_status_items(&items) {
                        Ok(path) => {
                            app.status_message =
                                Some(format!("Status line saved to {}", path.display()));
                        }
                        Err(err) => {
                            app.add_message(HistoryCell::System {
                                content: format!("Failed to save status line: {err}"),
                            });
                        }
                    }
                }
            }
            ViewEvent::HotbarSetupSaved { bindings } => {
                apply_hotbar_setup_saved(app, config, bindings);
            }
            ViewEvent::SetupStateCommitRequested { state, message } => match state.save() {
                Ok(()) => {
                    app.status_message = Some(message);
                }
                Err(err) => {
                    app.status_message = Some(format!("Setup state could not be saved: {err}"));
                }
            },
            ViewEvent::SetupConstitutionCommitRequested {
                constitution,
                state,
                message,
            } => match crate::tui::setup::persist_user_constitution_choice(&constitution, &state) {
                Ok(()) => {
                    app.status_message = Some(message);
                }
                Err(err) => {
                    app.status_message =
                        Some(format!("User constitution could not be saved: {err}"));
                }
            },
            ViewEvent::SetupConstitutionModelDraftRequested {
                draft,
                freeform_note,
                locale,
            } => {
                handle_setup_constitution_model_draft(app, config, draft, freeform_note, locale)
                    .await;
            }
            ViewEvent::FleetProfileModelDraftRequested {
                role,
                model,
                provider,
                reasoning_effort,
                locale,
            } => {
                handle_fleet_profile_model_draft(
                    app,
                    config,
                    role,
                    model,
                    provider,
                    reasoning_effort,
                    locale,
                )
                .await;
            }
            ViewEvent::FleetRosterOpenSetupRequested { member_id } => {
                // The shared router opens the selected v2 Fleet's exact editor
                // (focused on this member) or the legacy wizard when no named
                // Fleet is selected.
                open_fleet_setup_target(app, config, Some(&member_id));
            }
            ViewEvent::FleetRosterOpenModelRequested { member_id } => {
                open_fleet_model_target(app, config, &member_id);
            }
            ViewEvent::FleetListOpenDetailRequested { name, scope } => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetDetail) {
                    if let Some(view) = crate::tui::views::fleet_detail::FleetDetailView::open(
                        app, config, &name, scope,
                    ) {
                        app.view_stack.push(view);
                    } else {
                        app.set_sticky_status(
                            format!(
                                "Could not open Fleet `{name}` ({}) — the file may have moved or become unreadable.",
                                scope.label()
                            ),
                            crate::tui::app::StatusToastLevel::Error,
                            None,
                        );
                    }
                }
            }
            ViewEvent::FleetStoreChanged { message } => {
                app.status_message = Some(message);
                sync_fleet_roster(app, config, engine_handle);
            }
            ViewEvent::FleetRosterOpenFleetsRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::FleetList) {
                    app.view_stack
                        .push(crate::tui::views::fleet_list::FleetListView::new(
                            app, config,
                        ));
                }
            }
            ViewEvent::FleetRosterOpenWorkersRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::SubAgents) {
                    let agents = subagent_view_agents(app, &app.subagent_cache);
                    app.view_stack
                        .push(crate::tui::views::SubAgentsView::for_app(app, agents));
                }
                app.status_message =
                    Some(tr(app.ui_locale, MessageId::SubagentsFetching).to_string());
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            ViewEvent::FleetSetupExternalConsentActivationRequested { provider_id, model } => {
                // Validate the selected Fleet route by minting the read-only
                // external credential capability only for this exact
                // provider/source/path. The check is route-scoped: a cloned
                // config has the target provider active so credential discovery
                // succeeds, but the parent session provider/model are never
                // mutated.
                let Some(provider) = ApiProvider::parse(&provider_id) else {
                    app.set_sticky_status(
                        format!("Fleet route activation failed: unknown provider `{provider_id}`"),
                        crate::tui::app::StatusToastLevel::Error,
                        None,
                    );
                    app.needs_redraw = true;
                    continue;
                };
                let provider_label = provider.display_name();
                let mut scoped = config.clone();
                scoped.provider = Some(provider_id.clone());
                let validation =
                    crate::route_runtime::resolve_runtime_route(&scoped, provider, Some(&model))
                        .and_then(|route| route.validate().map_err(|err| err.to_string()));
                match validation {
                    Ok(validated) => {
                        app.provider_health
                            .record_success(&scoped, provider, &validated.model);
                        app.push_status_toast(
                            format!(
                                "{provider_label} route activated for Fleet: {}",
                                validated.model
                            ),
                            crate::tui::app::StatusToastLevel::Success,
                            Some(5_000),
                        );
                    }
                    Err(error) => {
                        let envelope = ErrorEnvelope::new(
                            ErrorCategory::Authentication,
                            ErrorSeverity::Error,
                            false,
                            "route_validation_failed",
                            &error,
                        );
                        app.provider_health
                            .record_failure(&scoped, provider, &model, &envelope);
                        app.push_status_toast(
                            format!("{provider_label} route activation failed: {error}"),
                            crate::tui::app::StatusToastLevel::Error,
                            None,
                        );
                    }
                }
                // Refresh the Fleet setup view from a snapshot built against the
                // updated health state so the activated row becomes Ready
                // without closing the modal.
                if app.view_stack.top_kind() == Some(crate::tui::views::ModalKind::FleetSetup)
                    && let Some(view) = app.view_stack.pop()
                {
                    let mut restored = view;
                    if let Some(fleet_setup) = restored
                        .as_any_mut()
                        .downcast_mut::<crate::tui::views::fleet_setup::FleetSetupView>(
                    ) {
                        let fresh = crate::tui::views::fleet_setup::FleetSetupSnapshot::from_app(
                            app, config,
                        );
                        fleet_setup.refresh_from_snapshot(fresh);
                    }
                    app.view_stack.push_boxed(restored);
                }
                app.needs_redraw = true;
            }
            ViewEvent::FleetProfileDraftCommitRequested { draft, scope } => {
                // A project-scope save is refused (never silently redirected)
                // when project profiles are disabled for this launch: the file
                // would be written where nothing loads it.
                if scope == crate::fleet::profile::FleetProfileScope::Project
                    && !crate::fleet::roster::project_agent_profiles_enabled()
                {
                    app.set_sticky_status(
                        tr(app.ui_locale, MessageId::FleetDestProjectDisabledSave).into_owned(),
                        StatusToastLevel::Error,
                        None,
                    );
                    app.needs_redraw = true;
                    continue;
                }
                // The TOML is rendered deterministically from the validated
                // draft and written atomically; the target path is derived
                // from the sanitized id, never model-chosen.
                let profile_dir =
                    match crate::fleet::profile::agent_profile_dir_for_scope(scope, &app.workspace)
                    {
                        Ok(dir) => dir,
                        Err(err) => {
                            app.set_sticky_status(
                                format!("Fleet {} scope is unavailable: {err:#}", scope.label()),
                                StatusToastLevel::Error,
                                None,
                            );
                            app.needs_redraw = true;
                            continue;
                        }
                    };
                let target = profile_dir.join(draft.file_name());
                // A ratified profile must not silently clobber a differently
                // named existing profile that shares this id (which would also
                // make the whole agents dir fail to load on the duplicate).
                // Overwriting the SAME file is fine — that is an intentional
                // re-draft of this profile.
                // The collision gate only needs file identities. Accept
                // otherwise legacy profile fields here so an old, unrelated
                // profile cannot block saving a current one. Malformed TOML,
                // unreadable files, and invalid ids still fail closed because
                // then we cannot prove there is no collision.
                let existing_profiles =
                    crate::fleet::profile::load_agent_profile_identities_from_dir(&profile_dir);
                if let Err(err) = &existing_profiles {
                    let message = tr(app.ui_locale, MessageId::FleetProfileIdentityVerifyFailed)
                        .replace("{error}", &format!("{err:#}"));
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                let id_conflict = existing_profiles
                    .into_iter()
                    .flatten()
                    .find(|p| p.id.eq_ignore_ascii_case(&draft.id) && p.source != target);
                if let Some(existing) = id_conflict {
                    let message = tr(app.ui_locale, MessageId::FleetProfileIdConflict)
                        .replace("{id}", &draft.id)
                        .replace("{path}", &existing.source.display().to_string());
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                // #4093 AC #5: a profile may only pin a provider the operator
                // has actually configured/credentialed. The picker already
                // offers models only from configured providers, but a
                // model-drafted or hand-edited route (or credentials removed
                // after the pick) could still name an unconfigured one — which
                // would fail loudly at launch. Catch it at save time with a
                // clear message, reusing the SAME predicate the picker uses.
                if let Some(provider_id) = draft.provider.as_deref()
                    && let Some(provider) = crate::config::ApiProvider::parse(provider_id)
                    && !crate::config::provider_is_configured_for_active(
                        config,
                        provider,
                        app.api_provider,
                    )
                {
                    let message = tr(app.ui_locale, MessageId::FleetProfileProviderUnconfigured)
                        .replace("{provider}", provider_id)
                        .replace("{env}", &provider.env_vars_label());
                    app.set_sticky_status(message, StatusToastLevel::Error, None);
                    app.needs_redraw = true;
                    continue;
                }
                let mut txn = codewhale_config::persistence::SetupTransaction::new();
                txn.stage(target.clone(), draft.render_toml().into_bytes());
                match txn.commit() {
                    Ok(()) => {
                        let roster =
                            std::sync::Arc::new(crate::fleet::identity::load_effective_roster(
                                &config.fleet_config(),
                                &app.workspace,
                                Some(app.plugin_registry.as_ref()),
                            ));
                        let roster_refresh_failed = engine_handle
                            .try_send(Op::SetFleetRoster { roster })
                            .is_err();
                        let zh = app.ui_locale == crate::localization::Locale::ZhHans;
                        app.add_message(HistoryCell::System {
                            content: if zh {
                                format!("已保存 Fleet 配置：{}", target.display())
                            } else {
                                format!(
                                    "Fleet {} profile saved: {}",
                                    scope.label(),
                                    target.display()
                                )
                            },
                        });
                        app.status_message = Some(if zh {
                            format!("已保存 Fleet 配置：{}", draft.file_name())
                        } else if roster_refresh_failed {
                            format!(
                                "Fleet {} profile saved, but the live roster could not refresh; restart before dispatching {}",
                                scope.label(),
                                draft.id
                            )
                        } else {
                            format!(
                                "Fleet {} profile saved: {}",
                                scope.label(),
                                draft.file_name()
                            )
                        });
                    }
                    Err(err) => {
                        app.status_message =
                            Some(if app.ui_locale == crate::localization::Locale::ZhHans {
                                format!("无法保存 Fleet 配置：{err:#}")
                            } else {
                                format!("Fleet profile could not be saved: {err:#}")
                            });
                    }
                }
                app.needs_redraw = true;
            }
            ViewEvent::SetupRuntimePresetApplyRequested {
                preset,
                state,
                message,
            } => match apply_setup_runtime_preset(app, config, preset, state) {
                Ok(summary) => {
                    sync_mode_update(app, engine_handle).await;
                    app.status_message = Some(format!("{message} {summary}"));
                }
                Err(err) => {
                    app.status_message =
                        Some(format!("Runtime preset could not be applied: {err:#}"));
                }
            },
            ViewEvent::SetupOpenProviderRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ProviderPicker) {
                    let runtime_status = query_provider_runtime_status(engine_handle).await;
                    app.view_stack.push(
                        crate::tui::provider_picker::ProviderPickerView::new_for_setup(
                            app.api_provider,
                            Some(app.api_provider),
                            config,
                            runtime_status,
                        )
                        .with_locale(app.ui_locale)
                        .with_provider_health(&app.provider_health),
                    );
                    app.status_message =
                        Some("Provider setup opened from /setup readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenModelRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ModelPicker) {
                    open_model_picker_for_provider(app, config, app.api_provider);
                    app.status_message =
                        Some("Model route picker opened from /setup readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenFleetRequested => {
                open_fleet_setup_target(app, config, None);
            }
            ViewEvent::SetupOpenHotbarRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::HotbarSetup) {
                    app.view_stack
                        .push(crate::tui::hotbar::setup::HotbarSetupView::new(app, config));
                    app.status_message =
                        Some("Hotbar setup opened from /setup Hotbar readiness.".to_string());
                }
            }
            ViewEvent::SetupOpenModeRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::ModePicker) {
                    app.view_stack
                        .push(crate::tui::views::mode_picker::ModePickerView::new(
                            app.mode,
                            app.ui_locale,
                        ));
                    app.status_message =
                        Some("Work mode picker opened from /setup runtime posture.".to_string());
                }
            }
            ViewEvent::SetupOpenConfigRequested => {
                if app.view_stack.top_kind() != Some(ModalKind::Config) {
                    app.view_stack.push(ConfigView::new_for_app(app));
                    app.status_message =
                        Some("Config view opened from /setup runtime posture.".to_string());
                }
            }
            ViewEvent::SetupOpenRemoteControlRequested => {
                start_remote_control_session(app, config);
            }
            ViewEvent::HotbarDisableRequested => {
                disable_hotbar(app, config);
            }
            ViewEvent::SubAgentsRefresh => {
                app.status_message = Some("Refreshing sub-agents...".to_string());
                // #3802: non-blocking send — refresh op, safe to drop.
                let _ = engine_handle.try_send(Op::ListSubAgents);
            }
            ViewEvent::SidebarAgentCancel { agent_id } => {
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
            ViewEvent::OpenAgentTranscript { agent_id } => {
                // One agent, one destination: focus the worker so its full
                // transcript owns the main area and the composer addresses
                // its fork. The register modal closes so the focus is visible.
                if app.view_stack.top_kind() == Some(ModalKind::SubAgents) {
                    app.view_stack.pop();
                }
                crate::tui::agent_focus::focus_agent(app, &agent_id);
                app.needs_redraw = true;
            }
            ViewEvent::AgentDetailsClosed { agent_id } => {
                crate::tui::work_surface::agent_details_closed(app, &agent_id);
            }
            ViewEvent::FilePickerSelected { path } => {
                // Insert `@<path>` at the composer's cursor with surrounding
                // whitespace so the existing `@`-mention parser picks it up.
                let cursor = app.cursor_position;
                let needs_leading_space = cursor > 0
                    && !app
                        .input
                        .chars()
                        .nth(cursor.saturating_sub(1))
                        .is_some_and(|c| c.is_whitespace());
                let mut insertion = String::new();
                if needs_leading_space {
                    insertion.push(' ');
                }
                insertion.push('@');
                insertion.push_str(&path);
                insertion.push(' ');
                app.insert_str(&insertion);
                app.status_message = Some(format!("Attached @{path}"));
            }
            ViewEvent::ModelPickerApplied {
                model,
                provider,
                provider_id,
                effort,
                previous_model,
                previous_effort,
                save_as_startup_default,
            } => {
                apply_model_picker_choice(
                    app,
                    engine_handle,
                    config,
                    model,
                    provider,
                    provider_id,
                    effort,
                    previous_model,
                    previous_effort,
                    save_as_startup_default,
                )
                .await;
            }
            ViewEvent::ModelPickerDismissed {
                catalog_view,
                view,
                selected_row_id,
            } => {
                sync_config_provider_from_app(config, app);
                app.model_picker_memory = Some(crate::tui::app::ModelPickerMemory {
                    catalog_view,
                    view: Some(view),
                    selected_row_id,
                });
            }
            ViewEvent::ModelPickerRefresh => {
                // Re-resolve readiness from the live credential state and
                // rebuild catalog rows. Non-destructive: never clears the list
                // when a refresh fails; just re-project from current config.
                sync_config_provider_from_app(config, app);
                if app.view_stack.top_kind() == Some(ModalKind::ModelPicker)
                    && let Some(mut boxed) = app.view_stack.pop()
                {
                    if let Some(picker) = boxed
                        .as_any_mut()
                        .downcast_mut::<crate::tui::model_picker::ModelPickerView>(
                    ) {
                        picker.re_resolve_from_app(app, config);
                        app.status_message =
                            Some("Model readiness refreshed · catalog rows rebuilt".into());
                    }
                    app.view_stack.push_boxed(boxed);
                } else {
                    app.status_message =
                        Some("Open /model to refresh readiness and catalog".into());
                }
                app.needs_redraw = true;
            }
            ViewEvent::ModelPickerToggleFleet {
                provider,
                provider_id,
                model,
            } => {
                use crate::fleet::members::{FleetModelChange, change_receipt, toggle_fleet_model};
                let provider_key = provider_id.unwrap_or_else(|| provider.as_str().to_string());
                let locale = app.ui_locale;
                // Same gate as `/fleet add`, against the live config: a
                // locked or unauthenticated provider row never enters the
                // fleet from the picker either.
                if let Some(rejection) =
                    crate::commands::fleet_provider_rejection(app, config, &provider_key)
                {
                    app.set_sticky_status(rejection, StatusToastLevel::Error, None);
                } else {
                    match toggle_fleet_model(&app.workspace, &provider_key, &model) {
                        Ok(change) => {
                            let level = if matches!(change, FleetModelChange::Unchanged { .. }) {
                                StatusToastLevel::Info
                            } else {
                                app.fleet_roster_stale = true;
                                StatusToastLevel::Success
                            };
                            app.push_status_toast(
                                change_receipt(locale, &provider_key, &model, &change),
                                level,
                                Some(FLEET_TOGGLE_TOAST_TTL_MS),
                            );
                        }
                        Err(error) => app.set_sticky_status(
                            tr(locale, MessageId::FleetToggleFailed)
                                .replace("{error}", &error.message(locale)),
                            StatusToastLevel::Error,
                            None,
                        ),
                    }
                }
                if let Some(mut boxed) = app.view_stack.pop() {
                    if let Some(picker) = boxed
                        .as_any_mut()
                        .downcast_mut::<crate::tui::model_picker::ModelPickerView>(
                    ) {
                        picker.re_resolve_from_app(app, config);
                    }
                    app.view_stack.push_boxed(boxed);
                }
                app.needs_redraw = true;
            }
            ViewEvent::ModelPickerTogglePin {
                provider,
                provider_id,
                model,
            } => {
                let provider_key = provider_id.unwrap_or_else(|| provider.as_str().to_string());
                match crate::settings::Settings::transact(|settings| {
                    Ok(settings.toggle_pinned_model(&provider_key, &model))
                }) {
                    Ok(true) => app.status_message = Some(format!("Pinned {provider_key}/{model}")),
                    Ok(false) => {
                        app.status_message = Some(format!("Unpinned {provider_key}/{model}"))
                    }
                    Err(error) => {
                        app.status_message = Some(format!("Could not update pin: {error}"))
                    }
                }
                if let Ok(settings) = crate::settings::Settings::load_persisted() {
                    app.pinned_models = settings.pinned_models;
                }
                if let Some(mut boxed) = app.view_stack.pop() {
                    if let Some(picker) = boxed
                        .as_any_mut()
                        .downcast_mut::<crate::tui::model_picker::ModelPickerView>(
                    ) {
                        picker.re_resolve_from_app(app, config);
                    }
                    app.view_stack.push_boxed(boxed);
                }
                app.needs_redraw = true;
            }
            ViewEvent::ModelPickerMovePin {
                provider,
                provider_id,
                model,
                delta,
            } => {
                let provider_key = provider_id.unwrap_or_else(|| provider.as_str().to_string());
                let reordered = crate::settings::Settings::transact_opt(|settings| {
                    if !settings.move_pinned_model(&provider_key, &model, delta) {
                        return Ok(None);
                    }
                    Ok(Some(settings.pinned_models.clone()))
                });
                match reordered {
                    Ok(None) => {}
                    Ok(Some(pinned_models)) => {
                        app.pinned_models = pinned_models;
                        app.status_message = Some("Pinned model order updated".into());
                        if let Some(mut boxed) = app.view_stack.pop() {
                            if let Some(picker) = boxed
                                .as_any_mut()
                                .downcast_mut::<crate::tui::model_picker::ModelPickerView>(
                            ) {
                                picker.re_resolve_from_app(app, config);
                            }
                            app.view_stack.push_boxed(boxed);
                        }
                    }
                    Err(error) => {
                        app.status_message = Some(format!("Could not reorder pin: {error}"));
                    }
                }
                app.needs_redraw = true;
            }
            ViewEvent::ModelPickerNeedsAuth {
                provider,
                model,
                reason,
            } => {
                app.status_message = Some(reason);
                // Close the model picker if it is still open, then hand off to
                // the provider auth flow for the locked model's provider.
                while app.view_stack.top_kind() == Some(ModalKind::ModelPicker) {
                    let _ = app.view_stack.pop();
                }
                if let Some(picker) =
                    crate::tui::provider_picker::ProviderPickerView::new_for_missing_auth(
                        app.api_provider,
                        provider,
                        config,
                        None,
                    )
                {
                    app.view_stack.push(picker);
                } else {
                    app.status_message = Some(format!(
                        "🔒 {model} needs {provider:?} credentials — open /provider to authenticate."
                    ));
                }
                app.needs_redraw = true;
            }
            ViewEvent::StatusMessage { message } => {
                app.status_message = Some(message);
                app.needs_redraw = true;
            }
            ViewEvent::TopbarRoutePickerRequested => {
                open_provider_picker(app, config, engine_handle).await;
            }
            ViewEvent::ProviderPickerDismissed {
                catalog_view,
                selected_provider_id,
            } => {
                let onboarding_provider_picker = app.onboarding == OnboardingState::Provider;
                // A picker preview must never become route authority. During
                // onboarding Esc is deliberately non-mutating: it returns to
                // Language without touching config or the onboarding marker.
                if !onboarding_provider_picker {
                    sync_config_provider_from_app(config, app);
                }
                app.provider_picker_memory = Some(crate::tui::app::ProviderPickerMemory {
                    catalog_view,
                    selected_provider_id,
                });
                if onboarding_provider_picker {
                    back_from_provider_onboarding(app);
                }
            }
            ViewEvent::ProviderPickerApplied {
                provider,
                provider_id,
            } => {
                if let Some(provider_id) = provider_id {
                    set_active_custom_provider_in_memory(config, &provider_id);
                }
                let model_override = provider_picker_model_override(app, config, provider);
                let switched =
                    switch_provider(app, engine_handle, config, provider, model_override).await;
                if switched && app.onboarding == OnboardingState::Provider {
                    complete_provider_picker_onboarding(app, provider);
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerApiKeySubmitted {
                provider,
                provider_id,
                api_key,
                base_url,
            } => {
                let identity = picker_provider_identity(config, provider, provider_id.as_deref())
                    .map_err(anyhow::Error::msg)?;
                apply_provider_picker_api_key(
                    app,
                    engine_handle,
                    config,
                    identity,
                    api_key,
                    base_url,
                )
                .await;
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerSetupConfirmed {
                provider,
                provider_id,
                api_key,
                model,
                context_window,
                base_url,
            } => {
                let identity = picker_provider_identity(config, provider, provider_id.as_deref())
                    .map_err(anyhow::Error::msg)?;
                let completed = apply_provider_picker_setup_confirmed(
                    app,
                    engine_handle,
                    config,
                    identity,
                    api_key,
                    model,
                    context_window,
                    base_url,
                )
                .await;
                if completed && app.onboarding == OnboardingState::Provider {
                    complete_provider_picker_onboarding(app, provider);
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerCustomProviderSubmitted {
                provider_id,
                base_url,
                model,
                api_key_env,
            } => {
                let switched = apply_provider_picker_custom_provider(
                    app,
                    engine_handle,
                    config,
                    provider_id,
                    base_url,
                    model,
                    api_key_env,
                )
                .await;
                complete_provider_picker_onboarding_if_switched(app, ApiProvider::Custom, switched);
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerXaiOAuthRequested => {
                let switched =
                    run_xai_device_login_from_tui(terminal, app, engine_handle, config).await?;
                complete_provider_picker_onboarding_if_switched(app, ApiProvider::Xai, switched);
            }
            ViewEvent::ProviderPickerChatgptOAuthRequested => {
                let switched =
                    run_chatgpt_pkce_login_from_tui(terminal, app, engine_handle, config).await?;
                complete_provider_picker_onboarding_if_switched(
                    app,
                    ApiProvider::OpenaiCodex,
                    switched,
                );
            }
            ViewEvent::ProviderPickerExternalConsentConfirmed {
                provider,
                consent_provider,
                source,
                path,
            } => match persist_external_credential_consent_for_at(
                app.config_path.as_deref(),
                config,
                provider,
                consent_provider,
                source,
                &path,
            ) {
                Ok(_) => {
                    let toast = app
                        .tr(MessageId::ProviderExternalGrantedToast)
                        .replace("{owner}", source.owner_label())
                        .replace("{provider}", provider.as_str());
                    app.push_status_toast(toast, StatusToastLevel::Success, Some(8_000));
                    let model_override = provider_picker_model_override(app, config, provider);
                    let switched =
                        switch_provider(app, engine_handle, config, provider, model_override).await;
                    // #4763: reusing an external CLI grant completes provider
                    // onboarding exactly like a submitted key or an applied
                    // route. Without this the picker closes on success and
                    // the user is returned to the provider step they just
                    // satisfied — the second half of the reported loop.
                    if switched && app.onboarding == OnboardingState::Provider {
                        complete_provider_picker_onboarding(app, provider);
                    }
                    refresh_config_view_if_open(app, "provider");
                }
                Err(error) => app.push_status_toast(
                    app.tr(MessageId::ProviderExternalSaveFailedToast)
                        .replace("{error}", &error.to_string()),
                    StatusToastLevel::Error,
                    None,
                ),
            },
            ViewEvent::ProviderPickerExternalConsentRevoked { provider } => {
                match revoke_external_credential_consent_for_at(
                    app.config_path.as_deref(),
                    config,
                    provider,
                ) {
                    Ok(_) => app.push_status_toast(
                        app.tr(MessageId::ProviderExternalRevokedToast)
                            .replace("{provider}", provider.as_str()),
                        StatusToastLevel::Success,
                        Some(5_000),
                    ),
                    Err(error) => app.push_status_toast(
                        app.tr(MessageId::ProviderExternalRevokeFailedToast)
                            .replace("{error}", &error.to_string()),
                        StatusToastLevel::Error,
                        None,
                    ),
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ProviderPickerOpenModels {
                provider,
                provider_id,
            } => {
                if let Some(provider_id) = provider_id {
                    set_active_custom_provider_in_memory(config, &provider_id);
                }
                open_model_picker_for_provider(app, config, provider);
            }
            ViewEvent::ProviderPickerTestConnection {
                provider,
                provider_id,
                catalog_view,
            } => {
                match picker_provider_identity(config, provider, provider_id.as_deref()) {
                    Ok(identity) => {
                        apply_provider_picker_test_connection(
                            app,
                            engine_handle,
                            config,
                            identity,
                            catalog_view,
                        )
                        .await;
                    }
                    Err(error) => {
                        app.push_status_toast(error, StatusToastLevel::Error, Some(8_000));
                    }
                }
                refresh_config_view_if_open(app, "provider");
            }
            ViewEvent::ModeSelected { mode } => {
                let prior_mode = app.mode;
                let msg = commands::switch_mode(app, mode);
                if app.mode != prior_mode {
                    sync_mode_update(app, engine_handle).await;
                }
                app.add_message(HistoryCell::System { content: msg });
            }
            ViewEvent::BacktrackStep { direction } => {
                app.backtrack.step(direction);
                if let Some(idx) = app.backtrack.selected_idx() {
                    update_backtrack_overlay_selection(app, idx);
                }
            }
            ViewEvent::BacktrackConfirm => {
                if let Some(depth) = app.backtrack.confirm() {
                    apply_backtrack(app, depth);
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
            ViewEvent::BacktrackCancel => {
                app.backtrack.reset();
                app.status_message = Some("Backtrack canceled".to_string());
                app.needs_redraw = true;
            }
            ViewEvent::ContextMenuSelected {
                action: ContextMenuAction::ExecuteCommand { command },
            } => {
                if execute_command_input(
                    terminal,
                    app,
                    engine_handle,
                    task_manager,
                    config,
                    &mut *web_config_session,
                    &command,
                )
                .await?
                {
                    return Ok(true);
                }
            }
            ViewEvent::ContextMenuSelected { action } => handle_context_menu_action(app, action),
            ViewEvent::SkillMutationRequested { request } => {
                handle_skill_mutation_requested(app, request).await;
            }
            ViewEvent::SkillsManagerToggleCompatible => {
                if app.view_stack.top_kind() == Some(ModalKind::SkillsManager)
                    && let Some(mut boxed) = app.view_stack.pop()
                {
                    if let Some(view) = boxed
                        .as_any_mut()
                        .downcast_mut::<crate::tui::views::skills_manager::SkillsManagerView>(
                    ) {
                        crate::tui::views::skills_manager::apply_toggle_compatible(view, app);
                    }
                    app.view_stack.push_boxed(boxed);
                }
            }
        }
    }

    Ok(false)
}

/// Keep the very large modal-event dispatcher out of the already-large TUI
/// loop future. Config previews take a dedicated small path: polling the full
/// dispatcher on top of the event loop exceeds the macOS main-thread stack in
/// debug builds before a theme preview can reach its next frame.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_view_events_boxed<'a>(
    terminal: &'a mut AppTerminal,
    app: &'a mut App,
    config: &'a mut Config,
    task_manager: &'a SharedTaskManager,
    engine_handle: &'a mut EngineHandle,
    web_config_session: &'a mut Option<WebConfigSession>,
    events: Vec<ViewEvent>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool>> + 'a>> {
    Box::pin(async move {
        for event in events {
            match event {
                ViewEvent::ConfigUpdated {
                    key,
                    value,
                    persist,
                } => {
                    if handle_config_updated(
                        terminal,
                        app,
                        config,
                        task_manager,
                        engine_handle,
                        web_config_session,
                        key,
                        value,
                        persist,
                    )
                    .await?
                    {
                        return Ok(true);
                    }
                }
                ViewEvent::ThemeSelectionUpdated { theme, persist } => {
                    if handle_theme_selection_updated(
                        terminal,
                        app,
                        config,
                        task_manager,
                        engine_handle,
                        web_config_session,
                        theme,
                        persist,
                    )
                    .await?
                    {
                        return Ok(true);
                    }
                }
                other => {
                    if Box::pin(handle_view_events(
                        terminal,
                        app,
                        config,
                        task_manager,
                        engine_handle,
                        web_config_session,
                        vec![other],
                    ))
                    .await?
                    {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    })
}
