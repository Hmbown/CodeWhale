//! `/title` command — set a distinct tab/window title for the current
//! session, shown as `[title] …` in front of the terminal window title.
//!
//! This is deliberately separate from `/rename`, which changes the session
//! *name* shown in the composer border and session picker. `/title` only
//! affects the terminal window/tab title, so parallel sessions — each in
//! its own terminal window — are identifiable at a glance while they are
//! reasoning, using tools, or waiting.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::session_manager::{SessionManager, update_session};
use crate::tui::app::App;

use super::CommandResult;

const MAX_TITLE_LEN: usize = 100;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "title",
    aliases: &["tabtitle", "window-title"],
    usage: "/title [new title|off]",
    description_id: MessageId::CmdTitleDescription,
};

pub(in crate::commands) struct TitleCmd;

impl RegisterCommand for TitleCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        set_window_title(app, arg)
    }
}

/// Set (or clear) the current session's tab/window title.
///
/// - `/title <new title>` — set the window-title prefix for this session and
///   persist it on the saved session so it survives restarts.
/// - `/title off` — clear the session-level title; the `title` config
///   default (if any) still applies.
/// - `/title` with no argument — report the current effective title.
pub fn set_window_title(app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).filter(|s| !s.is_empty());
    let Some(arg) = arg else {
        let current = app.window_title_prefix().unwrap_or("unset");
        let source = if app.window_title.is_some() {
            " (session)"
        } else if app.title_default.is_some() {
            " (config default)"
        } else {
            ""
        };
        return CommandResult::message(format!("Window title: [{current}]{source}"));
    };

    if arg == "off" || arg == "clear" || arg == "none" {
        let manager = match resolve_manager() {
            Ok(manager) => manager,
            Err(error) => return error,
        };
        return set_window_title_with_manager(app, None, &manager);
    }

    if arg.chars().count() > MAX_TITLE_LEN {
        return CommandResult::error(format!("Title too long (max {MAX_TITLE_LEN} characters)"));
    }

    let manager = match resolve_manager() {
        Ok(manager) => manager,
        Err(error) => return error,
    };
    set_window_title_with_manager(app, Some(arg.to_string()), &manager)
}

#[allow(clippy::result_large_err)]
fn resolve_manager() -> Result<SessionManager, CommandResult> {
    SessionManager::default_location()
        .map_err(|e| CommandResult::error(format!("Could not open sessions directory: {e}")))
}

/// Set or clear the session-level window title with an explicit manager.
///
/// Mirrors the `/rename` write path (snapshot Work state and live metadata
/// before saving so the write cannot revert unsaved messages), then updates
/// `app.window_title` in the same step the disk write lands.
pub(crate) fn set_window_title_with_manager(
    app: &mut App,
    title: Option<String>,
    manager: &SessionManager,
) -> CommandResult {
    let session_id = match &app.current_session_id {
        Some(id) => id.clone(),
        None => {
            return CommandResult::error(
                "No active session. Send a message first to start a session.",
            );
        }
    };

    let mut session = match manager.load_session(&session_id) {
        Ok(s) => s,
        Err(e) => return CommandResult::error(format!("Could not load session: {e}")),
    };

    // Sync with current App state to avoid overwriting unsaved messages.
    session = update_session(
        session,
        &app.api_messages,
        u64::from(app.session.total_tokens),
        app.system_prompt.as_ref(),
    );
    session.work_state = match app.work_state_snapshot() {
        Ok(state) => state,
        Err(err) => {
            return CommandResult::error(format!(
                "Could not snapshot Work state before setting title: {err}"
            ));
        }
    };
    session.context_references = app.session_context_references.clone();
    session.artifacts = app.session_artifacts.clone();
    session.last_auto_route = app.auto_route_for_persistence();
    session
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    session.metadata.workspace.clone_from(&app.workspace);
    session.metadata.mode = Some(app.mode.as_setting().to_string());
    app.sync_cost_to_metadata(&mut session.metadata);
    session.window_title = title.clone();

    match manager.save_session(&session) {
        Ok(_) => {
            app.window_title = title.clone();
            // The render loop syncs the resolved prefix into the terminal
            // title; force a frame so the change lands immediately.
            app.needs_redraw = true;
            if let Err(err) = app.publish_pending_work_state() {
                return CommandResult::error(format!(
                    "Window title saved, but Work views were not published: {err}"
                ));
            }
            match title {
                Some(title) => CommandResult::message(format!(
                    "Window title set to \"{title}\" — the terminal tab now reads [\"{title}\"] …"
                )),
                None => CommandResult::message(
                    "Window title cleared (the config default still applies if set)",
                ),
            }
        }
        Err(e) => CommandResult::error(format!("Could not save session: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::session_manager::{SessionManager, create_saved_session_with_mode};
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn make_app(tmpdir: &TempDir) -> App {
        App::new(
            TuiOptions {
                skills_dir: tmpdir.path().join("skills"),
                memory_path: tmpdir.path().join("memory.md"),
                notes_path: tmpdir.path().join("notes.txt"),
                mcp_config_path: tmpdir.path().join("mcp.json"),
                ..crate::test_support::test_tui_options(tmpdir.path())
            },
            &Config::default(),
        )
    }

    fn make_session_manager(tmpdir: &TempDir) -> SessionManager {
        SessionManager::new(tmpdir.path().join("sessions")).unwrap()
    }

    #[test]
    fn no_active_session_reports_usage_error() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        app.current_session_id = None;
        let result = set_window_title(&mut app, Some("task-7"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("No active session"));
    }

    #[test]
    fn set_title_persists_on_the_saved_session() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);
        let mut session = create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        session.metadata.id = "title-test".to_string();
        session.metadata.title = "Original Name".to_string();
        manager.save_session(&session).unwrap();
        app.current_session_id = Some("title-test".to_string());

        let result =
            set_window_title_with_manager(&mut app, Some("parallel-task".to_string()), &manager);
        assert!(!result.is_error, "unexpected error: {:?}", result.message);
        assert_eq!(app.window_title.as_deref(), Some("parallel-task"));
        // `/rename`-style session name stays untouched.
        assert_eq!(app.session_title, None);

        let reloaded = manager.load_session("title-test").unwrap();
        assert_eq!(reloaded.window_title.as_deref(), Some("parallel-task"));
        assert_eq!(reloaded.metadata.title, "Original Name");
    }

    #[test]
    fn clear_title_removes_the_session_level_title() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);
        let mut session = create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            None,
            Some("agent"),
        );
        session.metadata.id = "title-clear".to_string();
        session.window_title = Some("stale".to_string());
        manager.save_session(&session).unwrap();
        app.current_session_id = Some("title-clear".to_string());

        let result = set_window_title_with_manager(&mut app, None, &manager);
        assert!(!result.is_error, "unexpected error: {:?}", result.message);
        assert_eq!(app.window_title, None);
        assert_eq!(
            manager.load_session("title-clear").unwrap().window_title,
            None
        );
    }

    #[test]
    fn empty_argument_reports_the_effective_title() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        app.title_default = Some("workspace-x".to_string());
        let result = set_window_title(&mut app, None);
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("[workspace-x]"));

        app.window_title = Some("session-specific".to_string());
        let result = set_window_title(&mut app, None);
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("[session-specific]"));
    }

    #[test]
    fn oversized_title_is_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        app.current_session_id = Some("any".to_string());
        let long = "x".repeat(MAX_TITLE_LEN + 1);
        let result = set_window_title(&mut app, Some(&long));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("Title too long"));
    }
}
