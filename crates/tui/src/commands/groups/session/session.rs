//! Session commands: save, load, compact, export

use std::path::PathBuf;

use crate::session_manager::{
    create_saved_session_with_id_and_mode, create_saved_session_with_mode,
};
use crate::tui::app::{App, AppAction};
use crate::tui::session_picker::SessionPickerView;

use super::CommandResult;

/// Save session to file.
///
/// When an explicit path is given, the session is exported there
/// (user-visible explicit export).  Without a path, v0.8.44 saves
/// into the managed session directory (`~/.codewhale/sessions`
/// or legacy `~/.deepseek/sessions`) so repo-local `session_*.json`
/// artifacts are no longer created by default.
pub fn save(app: &mut App, path: Option<&str>) -> CommandResult {
    let explicit_save_path = path.map(PathBuf::from);

    let messages = app.api_messages.clone();
    let mut session = create_saved_session_with_mode(
        &messages,
        &app.model,
        &app.workspace,
        u64::from(app.session.total_tokens),
        app.system_prompt.as_ref(),
        Some(app.mode.label()),
    );
    session
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    app.sync_cost_to_metadata(&mut session.metadata);
    session.context_references = app.session_context_references.clone();
    session.artifacts = app.session_artifacts.clone();
    session.work_state = match app.work_state_snapshot() {
        Ok(state) => state,
        Err(err) => return CommandResult::error(format!("Failed to snapshot Work state: {err}")),
    };
    session.last_auto_route = app.auto_route_for_persistence();
    let save_path = explicit_save_path.unwrap_or_else(|| {
        let dir = crate::session_manager::default_sessions_dir()
            .unwrap_or_else(|_| app.workspace.clone());
        dir.join(format!("{}.json", session.metadata.id))
    });

    let sessions_dir = save_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| app.workspace.clone(), std::path::Path::to_path_buf);

    match std::fs::create_dir_all(&sessions_dir) {
        Ok(()) => {
            let json = match serde_json::to_string_pretty(&session) {
                Ok(j) => j,
                Err(e) => return CommandResult::error(format!("Failed to serialize session: {e}")),
            };
            match crate::utils::write_atomic(&save_path, json.as_bytes()) {
                Ok(()) => {
                    app.current_session_id = Some(session.metadata.id.clone());
                    app.current_session_metadata = Some(session.metadata.clone());
                    app.session_title = Some(session.metadata.title.clone());
                    if let Err(err) = app.publish_pending_work_state() {
                        return CommandResult::error(format!(
                            "Session saved, but Work views were not published: {err}"
                        ));
                    }
                    CommandResult::message(format!(
                        "Session saved to {} (ID: {})",
                        save_path.display(),
                        crate::session_manager::truncate_id(&session.metadata.id)
                    ))
                }
                Err(e) => CommandResult::error(format!("Failed to save session: {e}")),
            }
        }
        Err(e) => CommandResult::error(format!("Failed to create directory: {e}")),
    }
}

/// Fork a specific session by id/prefix into a new sibling session and switch to it.
/// This implements `/fork <session_id>` for picker-based forking (#576).
pub fn fork_from_session(app: &mut App, session_id_or_prefix: &str) -> CommandResult {
    if app.session_transition_blocked() {
        return CommandResult::error(
            "Cannot fork a session while runtime work is active. Wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work first.",
        );
    }
    let manager = match crate::session_manager::SessionManager::default_location() {
        Ok(m) => m,
        Err(err) => {
            return CommandResult::error(format!("could not open sessions directory: {err}"));
        }
    };
    let source = manager
        .load_session(session_id_or_prefix)
        .or_else(|_| manager.load_session_by_prefix(session_id_or_prefix));
    let mut source_session = match source {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!(
                "could not load session '{}': {e}",
                session_id_or_prefix
            ));
        }
    };
    source_session.ensure_journal();
    let journal = source_session.journal.clone().unwrap_or_else(|| {
        crate::session_tree::SessionJournal::from_messages(
            source_session.messages.clone(),
            source_session.metadata.spawn_depth,
        )
    });
    let forked_journal = journal.fork_from(None).unwrap_or_else(|_| {
        crate::session_tree::SessionJournal::with_spawn_depth(
            source_session.metadata.spawn_depth.saturating_add(1),
        )
    });
    let messages = forked_journal.to_messages();
    let mut forked = crate::session_manager::create_saved_session_with_id_and_mode(
        uuid::Uuid::new_v4().to_string(),
        &messages,
        &source_session.metadata.model,
        &app.workspace,
        source_session.metadata.total_tokens,
        source_session
            .system_prompt
            .as_ref()
            .map(|s| crate::models::SystemPrompt::Text(s.clone()))
            .as_ref(),
        source_session.metadata.mode.as_deref(),
    );
    forked.journal = Some(forked_journal);
    forked.leaf_id = forked.journal.as_ref().and_then(|j| j.leaf_id.clone());
    forked.messages = messages;
    forked.metadata.spawn_depth = forked.journal.as_ref().map(|j| j.spawn_depth).unwrap_or(0);
    forked.metadata.parent_session_id = Some(source_session.metadata.id.clone());
    forked.metadata.forked_from_message_count = Some(source_session.metadata.message_count);
    forked.metadata.set_model_provider_route(
        source_session.metadata.model_provider.as_str(),
        source_session.metadata.model_provider_id.as_deref(),
    );
    forked.metadata.copy_cost_from(&source_session.metadata);
    forked.context_references = source_session.context_references.clone();
    forked.artifacts = source_session.artifacts.clone();
    forked.work_state = source_session.work_state.clone();
    forked.last_auto_route = source_session.last_auto_route.clone();
    if let Err(err) = manager.save_session(&forked) {
        return CommandResult::error(format!("Failed to save forked session: {err}"));
    }
    app.current_session_id = Some(forked.metadata.id.clone());
    app.current_session_metadata = Some(forked.metadata.clone());
    app.session_title = Some(forked.metadata.title.clone());
    // A fork starts as its own session: no inherited tab/window title.
    app.window_title = None;
    let parent_label = crate::session_manager::truncate_id(&source_session.metadata.id).to_string();
    let fork_label = crate::session_manager::truncate_id(&forked.metadata.id).to_string();
    CommandResult::with_message_and_action(
        format!(
            "Forked session {parent_label} -> {fork_label} (spawn_depth {})",
            forked.metadata.spawn_depth
        ),
        AppAction::SyncSession {
            session_id: Some(forked.metadata.id.clone()),
            messages: forked.messages.clone(),
            system_prompt: forked
                .system_prompt
                .as_ref()
                .map(|s| crate::models::SystemPrompt::Text(s.clone())),
            model: forked.metadata.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

/// Fork the active conversation into a new saved sibling session and switch to it.
pub fn fork(app: &mut App) -> CommandResult {
    if app.session_transition_blocked() {
        return CommandResult::error(
            "Cannot fork a session while runtime work is active. Wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work first.",
        );
    }
    if app.api_messages.is_empty() {
        return CommandResult::error("Nothing to fork. Send or load a message first.");
    }

    let manager = match crate::session_manager::SessionManager::default_location() {
        Ok(manager) => manager,
        Err(err) => {
            return CommandResult::error(format!("could not open sessions directory: {err}"));
        }
    };

    let parent_id = app
        .current_session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let mut parent = create_saved_session_with_id_and_mode(
        parent_id,
        &app.api_messages,
        &app.model,
        &app.workspace,
        u64::from(app.session.total_tokens),
        app.system_prompt.as_ref(),
        Some(app.mode.label()),
    );
    parent
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    if let Some(cached) = app
        .current_session_metadata
        .as_ref()
        .filter(|metadata| metadata.id == parent.metadata.id)
    {
        parent.metadata.created_at = cached.created_at;
        parent.metadata.title.clone_from(&cached.title);
        parent
            .metadata
            .parent_session_id
            .clone_from(&cached.parent_session_id);
        parent.metadata.forked_from_message_count = cached.forked_from_message_count;
    }
    app.sync_cost_to_metadata(&mut parent.metadata);
    parent.context_references = app.session_context_references.clone();
    parent.artifacts = app.session_artifacts.clone();
    let work_state = match app.work_state_snapshot() {
        Ok(state) => state,
        Err(err) => return CommandResult::error(format!("Failed to snapshot Work state: {err}")),
    };
    parent.work_state = work_state.clone();
    parent.last_auto_route = app.auto_route_for_persistence();

    if let Err(err) = manager.save_session(&parent) {
        return CommandResult::error(format!("Failed to save parent session: {err}"));
    }

    let mut forked = create_saved_session_with_mode(
        &app.api_messages,
        &app.model,
        &app.workspace,
        u64::from(app.session.total_tokens),
        app.system_prompt.as_ref(),
        Some(app.mode.label()),
    );
    forked
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    forked.metadata.copy_cost_from(&parent.metadata);
    forked.metadata.spawn_depth = parent.metadata.spawn_depth.saturating_add(1);
    // Ensure journal for both sessions: parent already has one from factory, bump forked's journal depth
    if let Some(j) = forked.journal.as_mut() {
        j.spawn_depth = forked.metadata.spawn_depth;
    }
    if let Some(j) = parent.journal.as_mut() {
        j.spawn_depth = parent.metadata.spawn_depth;
    }
    forked.metadata.mark_forked_from(&parent.metadata);
    forked.context_references = app.session_context_references.clone();
    forked.artifacts = app.session_artifacts.clone();
    forked.work_state = work_state;
    forked.last_auto_route = app.auto_route_for_persistence();

    if let Err(err) = manager.save_session(&forked) {
        return CommandResult::error(format!("Failed to save forked session: {err}"));
    }
    if let Err(err) = app.publish_pending_work_state() {
        return CommandResult::error(format!(
            "Sessions saved, but Work views were not published: {err}"
        ));
    }

    app.current_session_id = Some(forked.metadata.id.clone());
    app.current_session_metadata = Some(forked.metadata.clone());
    app.session_title = Some(forked.metadata.title.clone());
    // A fork starts as its own session: no inherited tab/window title.
    app.window_title = None;
    let fork_id = forked.metadata.id.clone();
    let parent_label = crate::session_manager::truncate_id(&parent.metadata.id).to_string();
    let fork_label = crate::session_manager::truncate_id(&fork_id).to_string();

    CommandResult::with_message_and_action(
        format!("Forked session {parent_label} -> {fork_label}"),
        AppAction::SyncSession {
            session_id: Some(fork_id),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

/// Start a fresh saved session from the current TUI state.
pub fn new_session(app: &mut App, arg: Option<&str>) -> CommandResult {
    let force = match arg.map(str::trim).filter(|s| !s.is_empty()) {
        None => false,
        Some("--force" | "force") => true,
        Some(other) => {
            return CommandResult::error(format!(
                "Usage: /new [--force]\n\nUnknown argument: {other}"
            ));
        }
    };

    if app.session_transition_blocked() {
        return CommandResult::error(
            "Cannot start a new session while runtime work is active. Wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work. `/new --force` only discards draft or queued input.",
        );
    }

    if !force {
        let blockers = new_session_blockers(app);
        if !blockers.is_empty() {
            return CommandResult::error(format!(
                "Cannot start a new session while {}. Run `/new --force` to discard pending work and start a fresh session.",
                blockers.join(", ")
            ));
        }
    }

    let new_id = uuid::Uuid::new_v4().to_string();
    if !super::super::core::reset_conversation_state(app) {
        return CommandResult::error(
            "Could not start a new session because Work state is busy; retry in a moment.",
        );
    }
    app.clear_input();
    app.session_artifacts.clear();
    app.session_context_references.clear();
    app.tool_evidence.clear();
    app.current_session_id = Some(new_id.clone());
    app.current_session_metadata = None;
    app.session_title = Some(crate::session_manager::DEFAULT_SESSION_TITLE.to_string());
    // A new session has no tab/window title override yet; the `title`
    // config default still applies.
    app.window_title = None;
    app.scroll_to_bottom();

    CommandResult::with_message_and_action(
        format!(
            "Started new session {} (New Session). Previous sessions remain available via /resume.",
            crate::session_manager::truncate_id(&new_id)
        ),
        AppAction::SyncSession {
            session_id: Some(new_id),
            messages: Vec::new(),
            system_prompt: None,
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

fn new_session_blockers(app: &App) -> Vec<&'static str> {
    let mut blockers = Vec::new();
    if !app.input.trim().is_empty() {
        blockers.push("the composer has unsent text");
    }
    if !app.queued_messages.is_empty() || app.queued_draft.is_some() {
        blockers.push("queued messages are pending");
    }
    blockers
}

/// Load session from file
pub fn load(app: &mut App, path: Option<&str>) -> CommandResult {
    if app.session_transition_blocked() {
        return CommandResult::error(
            "Cannot load a session while runtime work is active. Wait for the current turn, maintenance, and background tasks to finish, or cancel that specific work first.",
        );
    }
    let load_path = if let Some(p) = path {
        if p.contains('/') || p.contains('\\') {
            PathBuf::from(p)
        } else {
            app.workspace.join(p)
        }
    } else {
        return CommandResult::error("Usage: /load <path>");
    };

    let content = match std::fs::read_to_string(&load_path) {
        Ok(c) => c,
        Err(e) => {
            return CommandResult::error(format!("Failed to read session file: {e}"));
        }
    };

    let _session: crate::session_manager::SavedSession = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to parse session file: {e}"));
        }
    };

    // The command layer only validates the file shape. The event loop reloads
    // Config once and applies the session plus route atomically before it
    // rebuilds or syncs the engine.
    // Success is reported only after the event loop re-reads live Config and
    // atomically applies the session route. Emitting it here would leave a
    // false receipt in the current transcript if that final validation fails.
    CommandResult::action(crate::tui::app::AppAction::LoadSession(load_path))
}

/// Trigger context compaction. An optional argument becomes the summary
/// focus (`/compact the auth refactor`), forwarded into the successor brief.
pub fn compact(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let focus = arg
        .map(str::trim)
        .filter(|focus| !focus.is_empty())
        .map(str::to_string);
    let receipt = match focus.as_deref() {
        Some(focus) => format!("Context compaction triggered (focus: {focus})..."),
        None => "Context compaction triggered...".to_string(),
    };
    CommandResult::with_message_and_action(receipt, AppAction::CompactContext { focus })
}

/// Trigger agent-driven context purging.
pub fn purge(_app: &mut App) -> CommandResult {
    CommandResult::with_message_and_action(
        "Agent context purge triggered...".to_string(),
        AppAction::PurgeContext,
    )
}

/// Open the session picker UI, or run a sub-action like
/// `prune <days>` for housekeeping (#406 phase-1.5).
pub fn sessions(app: &mut App, arg: Option<&str>) -> CommandResult {
    let trimmed = arg.unwrap_or("").trim();
    if trimmed.is_empty() {
        app.view_stack
            .push(SessionPickerView::new(&app.workspace, app.ui_locale));
        return CommandResult::ok();
    }

    let mut parts = trimmed.split_whitespace();
    let action = parts.next().unwrap_or("").to_ascii_lowercase();
    match action.as_str() {
        "prune" => prune(app, parts.next()),
        "show" | "list" | "picker" => {
            app.view_stack
                .push(SessionPickerView::new(&app.workspace, app.ui_locale));
            CommandResult::ok()
        }
        // `open` is what the sidebar Sessions rail dispatches (#2934): it
        // opens the existing picker preselected on a session rather than
        // resuming inline, so resume keeps its single implementation.
        "open" => open_session(app, parts.next()),
        "archive" => set_archived(app, parts.next(), true),
        "unarchive" | "restore" => set_archived(app, parts.next(), false),
        _ => CommandResult::error(format!(
            "unknown subcommand `{action}`. usage: /sessions [show|open <id>|archive <id>|unarchive <id>|prune <days>]"
        )),
    }
}

/// Open the session picker with `session_id` preselected.
fn open_session(app: &mut App, session_id: Option<&str>) -> CommandResult {
    let Some(session_id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return CommandResult::error("usage: /sessions open <session-id>");
    };
    app.view_stack.push(SessionPickerView::new_selecting(
        &app.workspace,
        app.ui_locale,
        session_id,
    ));
    CommandResult::ok()
}

/// Archive or restore a saved session.
///
/// Routes through [`crate::session_manager::SessionManager::set_session_archived`]
/// — the same writer the picker and `PATCH /v1/sessions/{id}` use — so all
/// three surfaces produce one durable lifecycle state.
fn set_archived(app: &mut App, session_id: Option<&str>, archived: bool) -> CommandResult {
    let verb = if archived { "archive" } else { "unarchive" };
    let Some(session_id) = session_id.map(str::trim).filter(|id| !id.is_empty()) else {
        return CommandResult::error(format!("usage: /sessions {verb} <session-id>"));
    };
    let manager = match crate::session_manager::SessionManager::default_location() {
        Ok(manager) => manager,
        Err(err) => {
            return CommandResult::error(format!("could not open sessions directory: {err}"));
        }
    };
    // `Owner`: this is the in-process interactive surface, and the block below
    // updates the live cached metadata in the same step.
    match manager.set_session_archived(
        session_id,
        archived,
        crate::session_manager::SessionMutator::Owner,
    ) {
        Ok(metadata) => {
            // Atomic with the write, from the app's point of view: nothing can
            // run between the manager call and this update, so the next
            // autosave already sees the new lifecycle state.
            if let Some(cached) = app.current_session_metadata.as_mut()
                && cached.id == metadata.id
            {
                cached.archived = metadata.archived;
            }
            CommandResult::message(format!(
                "{} session {} ({})",
                if archived { "Archived" } else { "Restored" },
                crate::session_manager::truncate_id(&metadata.id),
                metadata.title
            ))
        }
        Err(err) => CommandResult::error(format!("{verb} failed: {err}")),
    }
}

/// Prune persisted sessions older than `<days>` from
/// `~/.deepseek/sessions/`. Wraps
/// [`crate::session_manager::SessionManager::prune_sessions_older_than`]
/// so users can run a safe cleanup without leaving the TUI. Skips
/// the checkpoint subdirectory (the helper guarantees that already).
fn prune(app: &mut App, days_arg: Option<&str>) -> CommandResult {
    let days_str = match days_arg {
        Some(s) => s,
        None => {
            return CommandResult::error(
                "usage: /sessions prune <days>   (e.g. `/sessions prune 30` to drop sessions older than 30 days)",
            );
        }
    };
    let days: u64 = match days_str.parse() {
        Ok(n) if n > 0 => n,
        _ => {
            return CommandResult::error(format!(
                "expected a positive integer number of days, got `{days_str}`"
            ));
        }
    };

    let manager = match crate::session_manager::SessionManager::default_location() {
        Ok(m) => m,
        Err(err) => {
            return CommandResult::error(format!("could not open sessions directory: {err}"));
        }
    };

    let max_age = std::time::Duration::from_secs(days.saturating_mul(24 * 60 * 60));
    // Never prune the active session, even if its timestamp is stale (a
    // just-resumed session isn't re-saved until its first post-resume write).
    let keep = app.current_session_id.as_deref();
    match manager.prune_sessions_older_than_keeping(max_age, keep) {
        Ok(0) => CommandResult::message(format!("no sessions older than {days}d to prune")),
        Ok(n) => CommandResult::message(format!(
            "pruned {n} session{} older than {days}d",
            if n == 1 { "" } else { "s" }
        )),
        Err(err) => CommandResult::error(format!("prune failed: {err}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::Role;
    use crate::test_support::EnvVarGuard;
    use crate::tui::app::{App, AppMode, ReasoningEffort, TuiOptions, TurnCacheRecord};
    use crate::tui::history::HistoryCell;
    use std::time::Instant;
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            ..crate::test_support::test_tui_options(tmpdir.path())
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn test_save_creates_file_and_sets_session_id() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let save_path = tmpdir.path().join("test_session.json");

        let result = save(&mut app, Some(save_path.to_str().unwrap()));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Session saved to"));
        assert!(msg.contains("ID:"));
        assert!(app.current_session_id.is_some());
        assert!(save_path.exists());
    }

    #[test]
    fn save_preserves_artifact_registry() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let save_path = tmpdir.path().join("artifact_session.json");
        app.session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_call_big".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "artifact-session".to_string(),
                tool_call_id: "call-big".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 512_000,
                preview: "cargo test output".to_string(),
                storage_path: tmpdir.path().join("call-big.txt"),
            });

        let result = save(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        let saved: crate::session_manager::SavedSession =
            serde_json::from_str(&std::fs::read_to_string(save_path).unwrap()).unwrap();
        assert_eq!(saved.artifacts, app.session_artifacts);
    }

    #[test]
    fn save_preserves_latest_auto_route_receipt() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let save_path = tmpdir.path().join("auto_route_session.json");
        let receipt = crate::model_routing::AutoRouteReceipt {
            tier: crate::model_routing::AutoRouteTier::Fast,
            pair: crate::model_routing::AutoRoutePair {
                strong: crate::config::ZAI_GLM_5_2_MODEL.to_string(),
                fast: Some(crate::config::ZAI_GLM_5_TURBO_MODEL.to_string()),
            },
            scope: crate::model_routing::AutoRouteScope::ResolvedProvider,
            data_path: crate::model_routing::AutoRouteDataPath::LocalHeuristic,
            reason: crate::model_routing::AutoRouteReason::LocalHeuristic(
                crate::model_routing::AutoRouteHeuristicReason::ShortRequest,
            ),
        };
        app.set_model_selection("auto".to_string());
        app.last_effective_provider = Some(crate::config::ApiProvider::Zai);
        app.last_effective_provider_identity = Some("zai".to_string());
        app.last_effective_model = Some(crate::config::ZAI_GLM_5_TURBO_MODEL.to_string());
        app.last_auto_route_receipt = Some(receipt.clone());
        app.last_effective_reasoning_effort =
            Some(crate::tui::app::EffectiveReasoningEffort::ThinkingEnabledGranularityUnavailable);

        let result = save(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        let saved: crate::session_manager::SavedSession =
            serde_json::from_str(&std::fs::read_to_string(save_path).unwrap()).unwrap();
        let route = saved.last_auto_route.expect("latest Auto route");
        assert_eq!(route.provider, crate::config::ApiProvider::Zai);
        assert_eq!(route.provider_identity, "zai");
        assert_eq!(route.model, crate::config::ZAI_GLM_5_TURBO_MODEL);
        assert_eq!(route.receipt, receipt);
        assert_eq!(
            route.effective_reasoning_effort,
            Some(crate::work_graph::ReasoningEffortTier::ThinkingEnabledGranularityUnavailable)
        );
    }

    #[test]
    fn fork_saves_parent_and_switches_to_child_session() {
        let tmpdir = TempDir::new().unwrap();
        let _lock = crate::test_support::lock_test_env();
        let home = tmpdir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home_guard = EnvVarGuard::set("HOME", &home);
        let previous_home = home_guard.previous();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.set_provider_identity(crate::config::ApiProvider::Custom, "lm-studio");
        app.current_session_id = Some("parent-session".to_string());
        let mut cached_parent = create_saved_session_with_id_and_mode(
            "parent-session".to_string(),
            &[],
            &app.model,
            &app.workspace,
            0,
            None,
            Some(app.mode.label()),
        )
        .metadata;
        cached_parent.title = "Custom Parent".to_string();
        cached_parent.created_at = "2026-01-02T03:04:05Z"
            .parse()
            .expect("fixed parent timestamp");
        app.current_session_metadata = Some(cached_parent.clone());
        app.session_title = Some(cached_parent.title.clone());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "try another path".to_string(),
                cache_control: None,
            }],
        });
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add(
                "preserve fork Work".to_string(),
                crate::tools::todo::TodoStatus::InProgress,
            );
        }
        {
            let mut plan = app.plan_state.try_lock().expect("plan lock");
            plan.update(crate::tools::plan::UpdatePlanArgs {
                objective: Some("Fork without Work drift".to_string()),
                ..crate::tools::plan::UpdatePlanArgs::default()
            });
        }
        app.cycle_effort();
        let expected_work = app
            .work_state_snapshot()
            .expect("Work snapshot")
            .expect("graph-backed Work state");
        assert!(
            expected_work.graph.is_some(),
            "fork fixture must use a graph"
        );

        let result = fork(&mut app);

        assert!(!result.is_error, "{:?}", result.message);
        let new_id = app.current_session_id.clone().expect("fork session id");
        assert_ne!(new_id, "parent-session");
        assert!(result.message.as_deref().unwrap_or("").contains("Forked"));
        assert!(matches!(result.action, Some(AppAction::SyncSession { .. })));

        let manager = crate::session_manager::SessionManager::default_location().unwrap();
        let parent = manager
            .load_session("parent-session")
            .expect("parent saved");
        let child = manager.load_session(&new_id).expect("child saved");
        assert_eq!(parent.messages.len(), 1);
        assert_eq!(parent.metadata.model_provider, "custom");
        assert_eq!(
            parent.metadata.model_provider_id.as_deref(),
            Some("lm-studio")
        );
        assert_eq!(parent.metadata.title, cached_parent.title);
        assert_eq!(parent.metadata.created_at, cached_parent.created_at);
        assert_eq!(
            child.metadata.parent_session_id.as_deref(),
            Some("parent-session")
        );
        assert_eq!(child.metadata.forked_from_message_count, Some(1));
        assert_eq!(child.metadata.model_provider, "custom");
        assert_eq!(
            child.metadata.model_provider_id.as_deref(),
            Some("lm-studio")
        );
        assert_eq!(parent.work_state.as_ref(), Some(&expected_work));
        assert_eq!(child.work_state.as_ref(), Some(&expected_work));
        let cached_child = app
            .current_session_metadata
            .as_ref()
            .expect("child metadata cached");
        assert_eq!(cached_child.id, child.metadata.id);
        assert_eq!(cached_child.title, child.metadata.title);
        assert_eq!(cached_child.created_at, child.metadata.created_at);
        assert_eq!(
            cached_child.parent_session_id,
            child.metadata.parent_session_id
        );
        assert_eq!(
            app.session_title.as_deref(),
            Some(child.metadata.title.as_str())
        );
        drop(home_guard);
        assert_eq!(std::env::var_os("HOME"), previous_home);
    }

    #[test]
    fn fork_rejects_active_runtime_without_switching_sessions() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("parent-session".to_string());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "still running".to_string(),
                cache_control: None,
            }],
        });
        app.is_loading = true;

        let result = fork(&mut app);

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert_eq!(app.current_session_id.as_deref(), Some("parent-session"));
        assert_eq!(app.api_messages.len(), 1);
    }

    #[test]
    fn new_session_from_resumed_state_creates_distinct_empty_session() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.session_title = Some("Old Session".to_string());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "continue this thread".to_string(),
                cache_control: None,
            }],
        });
        app.add_message(HistoryCell::System {
            content: "old transcript".to_string(),
        });
        app.system_prompt = Some(crate::models::SystemPrompt::Text("old prompt".to_string()));
        app.session.total_tokens = 123;
        app.session.session_cost = 1.25;

        let result = new_session(&mut app, None);

        assert!(!result.is_error, "{:?}", result.message);
        let new_id = app.current_session_id.clone().expect("new session id");
        assert_ne!(new_id, "old-session");
        assert_eq!(app.session_title.as_deref(), Some("New Session"));
        assert!(app.api_messages.is_empty());
        assert!(app.history.is_empty());
        assert!(app.system_prompt.is_none());
        assert_eq!(app.session.total_tokens, 0);
        assert_eq!(app.session.session_cost, 0.0);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("/resume")
        );
        match result.action {
            Some(AppAction::SyncSession {
                session_id,
                messages,
                system_prompt,
                ..
            }) => {
                assert_eq!(session_id.as_deref(), Some(new_id.as_str()));
                assert!(messages.is_empty());
                assert!(system_prompt.is_none());
            }
            other => panic!("expected SyncSession action, got {other:?}"),
        }
    }

    #[test]
    fn new_session_blocks_unsent_input_without_force() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.input = "draft text".to_string();

        let result = new_session(&mut app, None);

        assert!(result.is_error);
        assert_eq!(app.current_session_id.as_deref(), Some("old-session"));
        assert_eq!(app.input, "draft text");
        assert!(result.action.is_none());
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("/new --force")
        );
    }

    #[test]
    fn new_session_force_discards_unsent_input() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.input = "draft text".to_string();

        let result = new_session(&mut app, Some("--force"));

        assert!(!result.is_error, "{:?}", result.message);
        assert_ne!(app.current_session_id.as_deref(), Some("old-session"));
        assert!(app.input.is_empty());
        assert!(matches!(result.action, Some(AppAction::SyncSession { .. })));
    }

    #[test]
    fn new_session_blocks_in_flight_turn_without_force() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.is_loading = true;

        let result = new_session(&mut app, None);

        assert!(result.is_error);
        assert_eq!(app.current_session_id.as_deref(), Some("old-session"));
        assert!(result.action.is_none());
    }

    #[test]
    fn new_session_force_cannot_detach_an_in_flight_turn() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![],
        });
        app.is_loading = true;
        app.runtime_turn_status = Some("in_progress".to_string());

        let result = new_session(&mut app, Some("--force"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert_eq!(app.current_session_id.as_deref(), Some("old-session"));
        assert_eq!(app.api_messages.len(), 1);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("only discards draft or queued input"))
        );
    }

    #[test]
    fn load_rejects_an_active_runtime_before_reading_or_mutating() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![],
        });
        app.task_panel.push(crate::tui::app::TaskPanelEntry {
            id: "queued-late-producer".to_string(),
            status: "queued".to_string(),
            prompt_summary: "queued".to_string(),
            duration_ms: None,
            kind: crate::tui::app::TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
            current_tool: None,
            role: None,
            files_touched: 0,
        });

        let result = load(&mut app, Some("does-not-exist.json"));

        assert!(result.is_error);
        assert!(result.action.is_none());
        assert_eq!(app.current_session_id.as_deref(), Some("old-session"));
        assert_eq!(app.api_messages.len(), 1);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("runtime work is active"))
        );
    }

    #[test]
    fn test_save_with_default_path_uses_managed_sessions_dir() {
        let tmpdir = TempDir::new().unwrap();
        let _lock = crate::test_support::lock_test_env();
        // Set CODEWHALE_HOME so the managed sessions directory lands inside the
        // temp dir rather than the real user home. Pre-create the directory so
        // resolve_state_dir picks it up instead of falling back to legacy.
        let home = tmpdir.path().join("home");
        let sessions_dir = home.join("sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let codewhale_home = EnvVarGuard::set("CODEWHALE_HOME", &home);
        let previous_codewhale_home = codewhale_home.previous();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = save(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        // Give it a moment to ensure file is written
        std::thread::sleep(std::time::Duration::from_millis(10));
        let entries: Vec<_> = if sessions_dir.exists() {
            std::fs::read_dir(&sessions_dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_name().to_string_lossy().ends_with(".json"))
                .collect()
        } else {
            Vec::new()
        };
        drop(codewhale_home);
        // Session should be saved to the managed dir, not the workspace root.
        assert!(
            !entries.is_empty(),
            "expected session file in {sessions_dir:?}, got none; msg: {msg}"
        );
        let session_id = app
            .current_session_id
            .as_deref()
            .expect("current session id");
        assert!(sessions_dir.join(format!("{session_id}.json")).exists());
        assert_eq!(std::env::var_os("CODEWHALE_HOME"), previous_codewhale_home);
    }

    #[test]
    fn test_save_serialization_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        // This should work normally since SavedSession is serializable
        // Testing error path would require mocking, which is complex
        let save_path = tmpdir.path().join("test.json");
        let result = save(&mut app, Some(save_path.to_str().unwrap()));
        assert!(result.message.is_some());
    }

    #[test]
    fn test_load_without_path_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = load(&mut app, None);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Usage: /load"));
    }

    #[test]
    fn test_load_nonexistent_file_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = load(&mut app, Some("nonexistent.json"));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Failed to read"));
    }

    #[test]
    fn test_load_invalid_json_returns_error() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let bad_file = tmpdir.path().join("bad.json");
        std::fs::write(&bad_file, "not valid json").unwrap();
        let result = load(&mut app, Some(bad_file.to_str().unwrap()));
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Failed to parse"));
    }

    #[test]
    fn test_load_valid_session_defers_state_restore_to_event_loop() {
        let tmpdir = TempDir::new().unwrap();
        let mut app1 = create_test_app_with_tmpdir(&tmpdir);
        // Set up some state to save
        app1.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "Hello".to_string(),
                cache_control: None,
            }],
        });
        app1.session.total_tokens = 500;
        app1.set_mode(AppMode::Plan);
        let save_path = tmpdir.path().join("test.json");
        save(&mut app1, Some(save_path.to_str().unwrap()));

        // Create new app and load
        let mut app2 = create_test_app_with_tmpdir(&tmpdir);
        app2.system_prompt = Some(crate::models::SystemPrompt::Text(
            "stale prompt from prior session".to_string(),
        ));
        app2.session_context_references
            .push(crate::session_manager::SessionContextReference {
                message_index: 0,
                reference: crate::tui::file_mention::ContextReference {
                    kind: crate::tui::file_mention::ContextReferenceKind::File,
                    source: crate::tui::file_mention::ContextReferenceSource::AtMention,
                    badge: "file".to_string(),
                    label: "stale.rs".to_string(),
                    target: tmpdir.path().join("stale.rs").display().to_string(),
                    included: true,
                    expanded: true,
                    detail: None,
                },
            });
        let result = load(&mut app2, Some(save_path.to_str().unwrap()));
        assert_eq!(result.message, None);
        assert!(app2.api_messages.is_empty());
        assert_eq!(app2.session.total_tokens, 0);
        assert!(app2.current_session_id.is_none());
        assert!(app2.system_prompt.is_some());
        assert_eq!(app2.session_context_references.len(), 1);
        assert!(matches!(
            result.action,
            Some(AppAction::LoadSession(path)) if path == save_path
        ));
    }

    #[test]
    fn explicit_save_persists_work_state_and_load_defers_application() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        {
            let mut todos = saved_app.todos.try_lock().expect("todos lock");
            todos.add(
                "persist me".to_string(),
                crate::tools::todo::TodoStatus::InProgress,
            );
        }
        {
            let mut plan = saved_app.plan_state.try_lock().expect("plan lock");
            plan.update(crate::tools::plan::UpdatePlanArgs {
                objective: Some("Resume exactly".to_string()),
                ..crate::tools::plan::UpdatePlanArgs::default()
            });
        }
        let expected = saved_app.work_state_snapshot().expect("snapshot");
        let save_path = tmpdir.path().join("work_state.json");
        let saved = save(&mut saved_app, Some(save_path.to_str().unwrap()));
        assert!(!saved.is_error, "{:?}", saved.message);

        let mut loaded_app = create_test_app_with_tmpdir(&tmpdir);
        let loaded = load(&mut loaded_app, Some(save_path.to_str().unwrap()));
        assert!(!loaded.is_error, "{:?}", loaded.message);
        assert_eq!(loaded_app.work_state_snapshot().expect("snapshot"), None);
        assert!(matches!(
            loaded.action,
            Some(AppAction::LoadSession(path)) if path == save_path
        ));
        let saved_session: crate::session_manager::SavedSession =
            serde_json::from_str(&std::fs::read_to_string(&save_path).expect("saved session file"))
                .expect("saved session JSON");
        assert_eq!(saved_session.work_state, expected);
    }

    #[test]
    fn new_session_is_all_or_nothing_when_work_state_is_busy() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![],
        });
        app.current_session_id = Some("current-session".to_string());
        let todos = app.todos.clone();
        let _held = todos.try_lock().expect("hold todos lock");

        let result = new_session(&mut app, Some("--force"));

        assert!(result.is_error);
        assert_eq!(app.api_messages.len(), 1);
        assert_eq!(app.current_session_id.as_deref(), Some("current-session"));
        assert!(result.action.is_none());
    }

    #[test]
    fn load_auto_model_session_defers_model_restore_to_event_loop() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.set_model_selection("auto".to_string());
        saved_app.last_effective_model = Some("deepseek-v4-flash".to_string());
        saved_app.last_effective_reasoning_effort = Some(
            crate::tui::app::EffectiveReasoningEffort::Tier(ReasoningEffort::Low),
        );
        let save_path = tmpdir.path().join("auto_model.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.set_model_selection("deepseek-v4-flash".to_string());
        app.reasoning_effort = ReasoningEffort::High;
        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        assert!(!app.auto_model);
        assert_eq!(app.model, "deepseek-v4-flash");
        assert_eq!(app.reasoning_effort, ReasoningEffort::High);
        assert!(matches!(
            result.action,
            Some(AppAction::LoadSession(path)) if path == save_path
        ));
    }

    #[test]
    fn load_defers_artifact_registry_restore_to_event_loop() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app
            .session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_call_big".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "artifact-session".to_string(),
                tool_call_id: "call-big".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 128,
                preview: "checking crate".to_string(),
                storage_path: tmpdir.path().join("call-big.txt"),
            });
        let save_path = tmpdir.path().join("artifact_load.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_stale".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "stale-session".to_string(),
                tool_call_id: "stale".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 1,
                preview: "stale".to_string(),
                storage_path: tmpdir.path().join("stale.txt"),
            });

        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        assert_eq!(app.session_artifacts.len(), 1);
        assert_eq!(app.session_artifacts[0].id, "art_stale");
        assert!(matches!(
            result.action,
            Some(AppAction::LoadSession(path)) if path == save_path
        ));
    }

    #[test]
    fn load_defers_telemetry_reset_to_event_loop() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "checkpoint".to_string(),
                cache_control: None,
            }],
        });
        saved_app.session.total_tokens = 500;
        let save_path = tmpdir.path().join("checkpoint.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.session.session_cost = 1.25;
        app.session.session_cost_cny = 9.13;
        app.session.subagent_cost = 0.75;
        app.session.subagent_cost_cny = 5.48;
        app.session
            .subagent_usage_sources
            .insert(crate::cost_status::usage_source_fingerprint(
                "response-test",
            ));
        app.session.displayed_cost_high_water = 2.0;
        app.session.displayed_cost_high_water_cny = 14.61;
        app.session.last_prompt_tokens = Some(120);
        app.session.last_completion_tokens = Some(35);
        app.session.last_prompt_cache_hit_tokens = Some(80);
        app.session.last_prompt_cache_miss_tokens = Some(40);
        app.session.last_reasoning_replay_tokens = Some(12);
        app.push_turn_cache_record(TurnCacheRecord {
            provider: None,
            provider_identity: None,
            model: None,
            auto_model: false,
            input_tokens: 120,
            output_tokens: 35,
            cache_hit_tokens: Some(80),
            cache_miss_tokens: Some(40),
            reasoning_replay_tokens: Some(12),
            cache_write_tokens: None,
            reasoning_tokens: None,
            cost_audit: None,
            recorded_at: Instant::now(),
        });

        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert_eq!(result.message, None);
        assert_eq!(app.session.total_tokens, 0);
        assert_eq!(app.session.session_cost, 1.25);
        assert_eq!(app.session.session_cost_cny, 9.13);
        assert_eq!(app.session.subagent_cost, 0.75);
        assert_eq!(app.session.subagent_cost_cny, 5.48);
        assert_eq!(app.session.turn_cache_history.len(), 1);
        assert!(matches!(
            result.action,
            Some(AppAction::LoadSession(path)) if path == save_path
        ));
    }

    #[test]
    fn test_compact_toggles_state() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = compact(&mut app, None);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("compaction") || msg.contains("Compact"));
        assert!(matches!(
            result.action,
            Some(AppAction::CompactContext { focus: None })
        ));
    }

    #[test]
    fn compact_command_forwards_a_trimmed_focus_argument() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = compact(&mut app, Some("  the auth refactor  "));
        assert!(matches!(
            result.action,
            Some(AppAction::CompactContext { focus: Some(ref focus) }) if focus == "the auth refactor"
        ));
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|msg| msg.contains("focus: the auth refactor")),
            "{result:?}"
        );

        // Whitespace-only arguments behave like no focus at all.
        let blank = compact(&mut app, Some("   "));
        assert!(matches!(
            blank.action,
            Some(AppAction::CompactContext { focus: None })
        ));
    }

    #[test]
    fn test_sessions_pushes_picker_view() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let initial_kind = app.view_stack.top_kind();

        let result = sessions(&mut app, None);
        assert_eq!(result.message, None);
        assert!(result.action.is_none());
        // View should have changed (session picker should be on top)
        assert_ne!(app.view_stack.top_kind(), initial_kind);
    }

    #[test]
    fn test_sessions_show_subcommand_pushes_picker_view() {
        // `/sessions show` and `/sessions list` are explicit aliases
        // for the no-arg picker form. Verify they don't fall through
        // to the prune branch.
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let initial_kind = app.view_stack.top_kind();
        let result = sessions(&mut app, Some("show"));
        assert_eq!(result.message, None);
        assert_ne!(app.view_stack.top_kind(), initial_kind);
    }

    #[test]
    fn test_sessions_prune_requires_days_argument() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = sessions(&mut app, Some("prune"));
        assert!(result.is_error);
        assert!(
            result.message.as_deref().unwrap_or("").contains("usage"),
            "expected usage hint: {:?}",
            result.message
        );
    }

    #[test]
    fn test_sessions_prune_rejects_non_positive_days() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        for bad in ["0", "-3", "abc", "3.14"] {
            let result = sessions(&mut app, Some(&format!("prune {bad}")));
            assert!(result.is_error, "expected error for `{bad}`");
        }
    }

    #[test]
    fn test_sessions_unknown_subcommand_errors() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = sessions(&mut app, Some("teleport"));
        assert!(result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or("")
                .contains("unknown subcommand"),
            "expected unknown-subcommand error: {:?}",
            result.message
        );
    }
}
