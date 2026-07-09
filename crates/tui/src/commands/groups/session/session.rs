//! Session commands: save, load, compact, export

use std::fmt::Write;
use std::path::PathBuf;

use crate::session_manager::{
    create_saved_session_with_id_and_mode, create_saved_session_with_mode,
};
use crate::tui::app::{App, AppAction, AppMode};
use crate::tui::history::{HistoryCell, history_cells_from_message};
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
    let save_path = if let Some(p) = path {
        PathBuf::from(p)
    } else {
        let dir = crate::session_manager::default_sessions_dir()
            .unwrap_or_else(|_| app.workspace.clone());
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        dir.join(format!("session_{timestamp}.json"))
    };

    let messages = app.api_messages.clone();
    let mut session = create_saved_session_with_mode(
        &messages,
        &app.model,
        &app.workspace,
        u64::from(app.session.total_tokens),
        app.system_prompt.as_ref(),
        Some(app.mode.label()),
    );
    session.metadata.model_provider = app.api_provider.as_str().to_string();
    app.sync_cost_to_metadata(&mut session.metadata);
    session.artifacts = app.session_artifacts.clone();

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
            match std::fs::write(&save_path, json) {
                Ok(()) => {
                    app.current_session_id = Some(session.metadata.id.clone());
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

/// Fork the active conversation into a new saved sibling session and switch to it.
pub fn fork(app: &mut App) -> CommandResult {
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
    parent.metadata.model_provider = app.api_provider.as_str().to_string();
    app.sync_cost_to_metadata(&mut parent.metadata);
    parent.artifacts = app.session_artifacts.clone();

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
    forked.metadata.model_provider = app.api_provider.as_str().to_string();
    forked.metadata.copy_cost_from(&parent.metadata);
    forked.metadata.mark_forked_from(&parent.metadata);

    if let Err(err) = manager.save_session(&forked) {
        return CommandResult::error(format!("Failed to save forked session: {err}"));
    }

    app.current_session_id = Some(forked.metadata.id.clone());
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
    super::super::core::reset_conversation_state(app);
    app.clear_input();
    app.session_artifacts.clear();
    app.session_context_references.clear();
    app.tool_evidence.clear();
    app.current_session_id = Some(new_id.clone());
    app.session_title = Some("New Session".to_string());
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
    if app.is_loading || app.runtime_turn_status.as_deref() == Some("in_progress") {
        blockers.push("a turn is in progress");
    }
    if app.is_compacting {
        blockers.push("context compaction is running");
    }
    if app.task_panel.iter().any(|task| task.status == "running") {
        blockers.push("background tasks are running");
    }
    blockers
}

/// Load session from file
pub fn load(app: &mut App, path: Option<&str>) -> CommandResult {
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

    let session: crate::session_manager::SavedSession = match serde_json::from_str(&content) {
        Ok(s) => s,
        Err(e) => {
            return CommandResult::error(format!("Failed to parse session file: {e}"));
        }
    };

    app.api_messages.clone_from(&session.messages);
    app.clear_history();
    let cells_to_add: Vec<_> = app
        .api_messages
        .iter()
        .flat_map(history_cells_from_message)
        .collect();
    app.extend_history(cells_to_add);
    app.mark_history_updated();
    app.viewport.transcript_selection.clear();
    if let Some(provider) = crate::config::ApiProvider::parse(&session.metadata.model_provider) {
        app.api_provider = provider;
        app.reasoning_effort = app.reasoning_effort.normalize_for_provider(provider);
    }
    app.set_model_selection(session.metadata.model.clone());
    app.update_model_compaction_budget();
    app.workspace.clone_from(&session.metadata.workspace);
    if let Some(mode) = session.metadata.mode.as_deref().and_then(AppMode::parse) {
        app.set_mode(mode);
    }
    app.session.total_tokens = u32::try_from(session.metadata.total_tokens).unwrap_or(u32::MAX);
    app.session.total_conversation_tokens = app.session.total_tokens;
    // Accumulated token breakdown is per-runtime-session; zero on load.
    app.session.reset_token_breakdown();
    app.session.session_cost = 0.0;
    app.session.session_cost_cny = 0.0;
    app.session.subagent_cost = 0.0;
    app.session.subagent_cost_cny = 0.0;
    app.session.subagent_cost_event_seqs.clear();
    app.session.displayed_cost_high_water = 0.0;
    app.session.displayed_cost_high_water_cny = 0.0;
    app.session.last_prompt_tokens = None;
    app.session.last_completion_tokens = None;
    app.session.last_output_throughput = None;
    app.session.last_prompt_cache_hit_tokens = None;
    app.session.last_prompt_cache_miss_tokens = None;
    app.session.last_reasoning_replay_tokens = None;
    app.session.turn_cache_history.clear();
    app.current_session_id = Some(session.metadata.id.clone());
    app.session_artifacts = session.artifacts.clone();
    if let Some(sp) = session.system_prompt {
        app.system_prompt = Some(crate::models::SystemPrompt::Text(sp));
    }
    app.scroll_to_bottom();

    CommandResult::with_message_and_action(
        format!(
            "Session loaded from {} (ID: {}, {} messages)",
            load_path.display(),
            crate::session_manager::truncate_id(&session.metadata.id),
            session.metadata.message_count
        ),
        crate::tui::app::AppAction::SyncSession {
            session_id: app.current_session_id.clone(),
            messages: app.api_messages.clone(),
            system_prompt: app.system_prompt.clone(),
            model: app.model.clone(),
            workspace: app.workspace.clone(),
            mode: app.mode,
        },
    )
}

/// Trigger context compaction
pub fn compact(_app: &mut App) -> CommandResult {
    // Trigger immediate compaction via engine
    CommandResult::with_message_and_action(
        "Context compaction triggered...".to_string(),
        AppAction::CompactContext,
    )
}

/// Trigger agent-driven context purging.
pub fn purge(_app: &mut App) -> CommandResult {
    CommandResult::with_message_and_action(
        "Agent context purge triggered...".to_string(),
        AppAction::PurgeContext,
    )
}

/// Export conversation to markdown.
///
/// `/export turn [path]` is a distinct sub-mode (issue #4108): it produces a
/// compact, pasteable Markdown *handoff* of the current/latest turn — reusing
/// the Turn Inspector's (#4104) turn scope + section data — and copies it to the
/// clipboard by default (writing to `path` instead when one is given). Every
/// other invocation is the existing full-transcript Markdown export.
pub fn export(app: &mut App, path: Option<&str>) -> CommandResult {
    if let Some(arg) = path {
        let mut parts = arg.splitn(2, char::is_whitespace);
        let first = parts.next().unwrap_or("");
        if first.eq_ignore_ascii_case("turn") {
            let dest = parts.next().map(str::trim).filter(|s| !s.is_empty());
            return export_turn_handoff(app, dest);
        }
    }

    let export_path = path.map_or_else(
        || {
            let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
            PathBuf::from(format!("chat_export_{timestamp}.md"))
        },
        PathBuf::from,
    );

    let mut content = String::new();
    content.push_str("# Chat Export\n\n");
    let _ = write!(
        content,
        "**Model:** {}\n**Workspace:** {}\n**Date:** {}\n\n---\n\n",
        app.model,
        app.workspace.display(),
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );

    for cell in &app.history {
        let (role, body) = match cell {
            HistoryCell::User { content } => ("**You:**", content.clone()),
            HistoryCell::Assistant { content, .. } => ("**Assistant:**", content.clone()),
            HistoryCell::System { content } => ("*System:*", content.clone()),
            HistoryCell::Error { message, severity } => match severity {
                crate::error_taxonomy::ErrorSeverity::Warning => ("**Warning:**", message.clone()),
                crate::error_taxonomy::ErrorSeverity::Info => ("*Info:*", message.clone()),
                _ => ("**Error:**", message.clone()),
            },
            HistoryCell::Thinking { content, .. } => ("*Thinking:*", content.clone()),
            HistoryCell::Tool(tool) => ("**Tool:**", render_tool_cell(tool, 80)),
            HistoryCell::SubAgent(sub) => ("**Sub-agent:**", render_subagent_cell(sub, 80)),
            HistoryCell::ArchivedContext {
                level,
                range,
                summary,
                ..
            } => (
                "**Archived Context:**",
                format!("L{level} [{range}]: {summary}"),
            ),
        };

        let _ = write!(content, "{}\n\n{}\n\n---\n\n", role, body.trim());
    }

    match std::fs::write(&export_path, content) {
        Ok(()) => CommandResult::message(format!("Exported to {}", export_path.display())),
        Err(e) => CommandResult::error(format!("Failed to export: {e}")),
    }
}

/// Produce the compact turn handoff (issue #4108) and surface it the way the
/// app already surfaces exports.
///
/// Without a `dest`, the Markdown is copied to the system clipboard so it is
/// immediately pasteable into a PR/issue/Slack/next session (the primary intent
/// of the issue); if the clipboard is unavailable it falls back to a timestamped
/// file so the artifact is never lost. With a `dest`, the Markdown is written
/// there — matching `/export`'s file-write convention. The Markdown itself is
/// assembled by [`crate::tui::ui::turn_handoff_markdown`], which reuses the Turn
/// Inspector's turn scope and per-section data.
fn export_turn_handoff(app: &mut App, dest: Option<&str>) -> CommandResult {
    let markdown = crate::tui::ui::turn_handoff_markdown(app);

    if let Some(dest) = dest {
        let path = PathBuf::from(dest);
        return match std::fs::write(&path, &markdown) {
            Ok(()) => CommandResult::message(format!("Turn handoff written to {}", path.display())),
            Err(e) => CommandResult::error(format!("Failed to write turn handoff: {e}")),
        };
    }

    if app.clipboard.write_text(&markdown).is_ok() {
        let lines = markdown.lines().count();
        return CommandResult::message(format!(
            "Turn handoff copied to clipboard ({lines} lines) — paste into a PR, issue, or Slack"
        ));
    }

    // Clipboard unavailable (e.g. headless host): persist the artifact so the
    // handoff is still recoverable rather than silently lost.
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let path = PathBuf::from(format!("turn_handoff_{timestamp}.md"));
    match std::fs::write(&path, &markdown) {
        Ok(()) => CommandResult::message(format!(
            "Clipboard unavailable — turn handoff written to {}",
            path.display()
        )),
        Err(e) => {
            CommandResult::error(format!("Turn handoff clipboard and file write failed: {e}"))
        }
    }
}

/// Open the session picker UI, or run a sub-action like
/// `prune <days>` for housekeeping (#406 phase-1.5).
pub fn sessions(app: &mut App, arg: Option<&str>) -> CommandResult {
    let trimmed = arg.unwrap_or("").trim();
    if trimmed.is_empty() {
        app.view_stack.push(SessionPickerView::new(&app.workspace));
        return CommandResult::ok();
    }

    let mut parts = trimmed.split_whitespace();
    let action = parts.next().unwrap_or("").to_ascii_lowercase();
    match action.as_str() {
        "prune" => prune(app, parts.next()),
        "show" | "list" | "picker" => {
            app.view_stack.push(SessionPickerView::new(&app.workspace));
            CommandResult::ok()
        }
        _ => CommandResult::error(format!(
            "unknown subcommand `{action}`. usage: /sessions [show|prune <days>]"
        )),
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

fn render_tool_cell(tool: &crate::tui::history::ToolCell, width: u16) -> String {
    tool.lines(width)
        .into_iter()
        .map(line_to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_subagent_cell(cell: &crate::tui::history::SubAgentCell, width: u16) -> String {
    cell.lines(width)
        .into_iter()
        .map(line_to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_to_string(line: ratatui::text::Line<'static>) -> String {
    line.spans
        .into_iter()
        .map(|span| span.content.to_string())
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DEFAULT_TEXT_MODEL};
    use crate::test_support::EnvVarGuard;
    use crate::tui::app::{App, AppMode, ReasoningEffort, TuiOptions, TurnCacheRecord};
    use std::time::Instant;
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
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
    fn fork_saves_parent_and_switches_to_child_session() {
        let tmpdir = TempDir::new().unwrap();
        let _lock = crate::test_support::lock_test_env();
        let home = tmpdir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home_guard = EnvVarGuard::set("HOME", &home);
        let previous_home = home_guard.previous();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("parent-session".to_string());
        app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
            content: vec![crate::models::ContentBlock::Text {
                text: "try another path".to_string(),
                cache_control: None,
            }],
        });

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
        assert_eq!(
            child.metadata.parent_session_id.as_deref(),
            Some("parent-session")
        );
        assert_eq!(child.metadata.forked_from_message_count, Some(1));
        drop(home_guard);
        assert_eq!(std::env::var_os("HOME"), previous_home);
    }

    #[test]
    fn new_session_from_resumed_state_creates_distinct_empty_session() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.current_session_id = Some("old-session".to_string());
        app.session_title = Some("Old Session".to_string());
        app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
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
                .filter(|e| e.file_name().to_string_lossy().starts_with("session_"))
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
    fn test_load_valid_session_restores_state() {
        let tmpdir = TempDir::new().unwrap();
        let mut app1 = create_test_app_with_tmpdir(&tmpdir);
        // Set up some state to save
        app1.api_messages.push(crate::models::Message {
            role: "user".to_string(),
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
        let result = load(&mut app2, Some(save_path.to_str().unwrap()));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Session loaded from"));
        assert!(msg.contains("ID:"));
        assert!(msg.contains("messages"));
        assert_eq!(app2.api_messages.len(), 1);
        assert_eq!(app2.session.total_tokens, 500);
        assert_eq!(app2.mode, AppMode::Plan);
        assert!(app2.current_session_id.is_some());
        assert!(matches!(
            result.action,
            Some(AppAction::SyncSession {
                mode: AppMode::Plan,
                ..
            })
        ));
    }

    #[test]
    fn load_auto_model_session_restores_auto_mode() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.set_model_selection("auto".to_string());
        saved_app.last_effective_model = Some("deepseek-v4-flash".to_string());
        saved_app.last_effective_reasoning_effort = Some(ReasoningEffort::Low);
        let save_path = tmpdir.path().join("auto_model.json");
        save(&mut saved_app, Some(save_path.to_str().unwrap()));

        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.set_model_selection("deepseek-v4-flash".to_string());
        app.reasoning_effort = ReasoningEffort::High;
        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(!result.is_error);
        assert!(app.auto_model);
        assert_eq!(app.model, "auto");
        assert_eq!(app.model_selection_for_persistence(), "auto");
        assert_eq!(app.last_effective_model, None);
        assert_eq!(app.last_effective_reasoning_effort, None);
        assert_eq!(app.reasoning_effort, ReasoningEffort::Auto);
        assert_eq!(app.effective_model_for_budget(), DEFAULT_TEXT_MODEL);
    }

    #[test]
    fn load_restores_artifact_registry() {
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
        assert_eq!(app.session_artifacts, saved_app.session_artifacts);
    }

    #[test]
    fn load_resets_cache_history_and_cost() {
        let tmpdir = TempDir::new().unwrap();
        let mut saved_app = create_test_app_with_tmpdir(&tmpdir);
        saved_app.api_messages.push(crate::models::Message {
            role: "user".to_string(),
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
        app.session.subagent_cost_event_seqs.insert(42);
        app.session.displayed_cost_high_water = 2.0;
        app.session.displayed_cost_high_water_cny = 14.61;
        app.session.last_prompt_tokens = Some(120);
        app.session.last_completion_tokens = Some(35);
        app.session.last_prompt_cache_hit_tokens = Some(80);
        app.session.last_prompt_cache_miss_tokens = Some(40);
        app.session.last_reasoning_replay_tokens = Some(12);
        app.push_turn_cache_record(TurnCacheRecord {
            provider: None,
            model: None,
            auto_model: false,
            input_tokens: 120,
            output_tokens: 35,
            cache_hit_tokens: Some(80),
            cache_miss_tokens: Some(40),
            reasoning_replay_tokens: Some(12),
            recorded_at: Instant::now(),
        });

        let result = load(&mut app, Some(save_path.to_str().unwrap()));

        assert!(result.message.is_some());
        assert_eq!(app.session.total_tokens, 500);
        assert_eq!(app.session.total_conversation_tokens, 500);
        assert_eq!(app.session.session_cost, 0.0);
        assert_eq!(app.session.session_cost_cny, 0.0);
        assert_eq!(app.session.subagent_cost, 0.0);
        assert_eq!(app.session.subagent_cost_cny, 0.0);
        assert!(app.session.subagent_cost_event_seqs.is_empty());
        assert_eq!(app.session.displayed_cost_high_water, 0.0);
        assert_eq!(app.session.displayed_cost_high_water_cny, 0.0);
        assert_eq!(app.session.last_prompt_tokens, None);
        assert_eq!(app.session.last_completion_tokens, None);
        assert_eq!(app.session.last_prompt_cache_hit_tokens, None);
        assert_eq!(app.session.last_prompt_cache_miss_tokens, None);
        assert_eq!(app.session.last_reasoning_replay_tokens, None);
        assert!(app.session.turn_cache_history.is_empty());
    }

    #[test]
    fn test_compact_toggles_state() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);

        let result = compact(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("compaction") || msg.contains("Compact"));
        assert!(matches!(result.action, Some(AppAction::CompactContext)));
    }

    #[test]
    fn test_export_crees_markdown_file() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.history.push(HistoryCell::User {
            content: "Hello".to_string(),
        });
        app.history.push(HistoryCell::Assistant {
            content: "Hi there".to_string(),
            streaming: false,
        });

        let export_path = tmpdir.path().join("export.md");
        let result = export(&mut app, Some(export_path.to_str().unwrap()));
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Exported to"));
        assert!(export_path.exists());

        let content = std::fs::read_to_string(&export_path).unwrap();
        assert!(content.contains("# Chat Export"));
        assert!(content.contains("**Model:**"));
        assert!(content.contains("**You:**"));
        assert!(content.contains("**Assistant:**"));
    }

    #[test]
    fn export_turn_writes_compact_handoff_to_path() {
        // `/export turn <path>` (issue #4108) writes the compact turn handoff
        // to a file rather than copying to the clipboard, so this stays
        // deterministic without touching the system clipboard.
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        app.history.push(HistoryCell::User {
            content: "Fix the flaky login test".to_string(),
        });
        app.history.push(HistoryCell::Tool(
            crate::tui::history::ToolCell::PatchSummary(crate::tui::history::PatchSummaryCell {
                path: "src/login.rs".to_string(),
                summary: "guard against empty token".to_string(),
                status: crate::tui::history::ToolStatus::Success,
                error: None,
            }),
        ));
        app.history.push(HistoryCell::Assistant {
            content: "Fixed the race in the login test.".to_string(),
            streaming: false,
        });
        app.runtime_turn_status = Some("completed".to_string());

        let out_path = tmpdir.path().join("handoff.md");
        let result = export(&mut app, Some(&format!("turn {}", out_path.display())));

        assert!(!result.is_error, "{:?}", result.message);
        assert!(
            result
                .message
                .as_deref()
                .unwrap_or_default()
                .contains("Turn handoff written to"),
            "{:?}",
            result.message
        );
        let md = std::fs::read_to_string(&out_path).unwrap();
        assert!(md.contains("# Turn handoff"), "{md}");
        assert!(md.contains("## Intent"), "{md}");
        assert!(md.contains("Fix the flaky login test"), "{md}");
        assert!(md.contains("## Files changed"), "{md}");
        assert!(md.contains("src/login.rs"), "{md}");
        assert!(md.contains("## Result / status"), "{md}");
        assert!(
            md.contains("Result: Fixed the race in the login test."),
            "{md}"
        );
    }

    #[test]
    fn test_export_with_default_path() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = export(&mut app, None);
        assert!(result.message.is_some());
        // Should create file with timestamp name in current dir
        let entries: Vec<_> = std::fs::read_dir(".")
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("chat_export_"))
            .collect();
        // Clean up
        for entry in &entries {
            let _ = std::fs::remove_file(entry.path());
        }
        assert!(!entries.is_empty() || result.message.unwrap().contains("Exported to"));
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
