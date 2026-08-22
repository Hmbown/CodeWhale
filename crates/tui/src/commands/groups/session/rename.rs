//! `/rename` command — set a custom title for the current session.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::session_manager::{SessionManager, update_session};
use crate::tui::app::App;

use super::CommandResult;

const MAX_TITLE_LEN: usize = 100;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "rename",
    aliases: &["gaiming", "chongmingming"],
    usage: "/rename <new title>",
    description_id: MessageId::CmdRenameDescription,
};

pub(in crate::commands) struct RenameCmd;

impl RegisterCommand for RenameCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        rename(app, arg)
    }
}

/// Rename the current session to the given title.
///
/// Usage: `/rename <new title>`
///
/// The new title is persisted immediately to `~/.deepseek/sessions/<id>.json`
/// so the updated name is visible the next time the session picker is opened.
pub fn rename(app: &mut App, arg: Option<&str>) -> CommandResult {
    // Same character policy as the picker and Runtime API rename: controls
    // and bidi/zero-width format characters never reach the persisted title.
    let sanitized = arg
        .map(crate::session_manager::sanitize_session_title)
        .unwrap_or_default();
    let new_title = match Some(sanitized.trim()).filter(|s| !s.is_empty()) {
        Some(t) => t,
        None => return CommandResult::error("Usage: /rename <new title>"),
    };

    if new_title.chars().count() > MAX_TITLE_LEN {
        return CommandResult::error(format!("Title too long (max {MAX_TITLE_LEN} characters)"));
    }

    let session_id = match &app.current_session_id {
        Some(id) => id.clone(),
        None => {
            return CommandResult::error(
                "No active session. Send a message first to start a session.",
            );
        }
    };

    let manager = match SessionManager::default_location() {
        Ok(m) => m,
        Err(e) => return CommandResult::error(format!("Could not open sessions directory: {e}")),
    };

    rename_with_manager(new_title, &session_id, &manager, app)
}

pub(crate) fn rename_with_manager(
    new_title: &str,
    session_id: &str,
    manager: &SessionManager,
    app: &mut App,
) -> CommandResult {
    // Same character policy as the picker and Runtime API rename: controls
    // and bidi/zero-width format characters never reach the persisted title.
    let sanitized = crate::session_manager::sanitize_session_title(new_title);
    let new_title = sanitized.trim();
    if new_title.is_empty() {
        return CommandResult::error("Usage: /rename <new title>");
    }
    let mut session = match manager.load_session(session_id) {
        Ok(s) => s,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            match live_session_before_first_snapshot(manager, session_id, app) {
                Some(s) => s,
                None => {
                    return CommandResult::error(format!("Could not load session: {err}"));
                }
            }
        }
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
                "Could not snapshot Work state before rename: {err}"
            ));
        }
    };
    session.context_references = app.session_context_references.clone();
    session.artifacts = app.session_artifacts.clone();
    session.last_auto_route = app.auto_route_for_persistence();
    session.metadata.model = app.model_selection_for_persistence();
    session
        .metadata
        .set_model_provider_route(app.api_provider.as_str(), app.provider_id_for_persistence());
    session.metadata.workspace.clone_from(&app.workspace);
    session.metadata.mode = Some(app.mode.as_setting().to_string());
    app.sync_cost_to_metadata(&mut session.metadata);
    session.metadata.title = new_title.to_string();

    match manager.save_session(&session) {
        Ok(_) => {
            app.current_session_metadata = Some(session.metadata.clone());
            app.session_title = Some(new_title.to_string());
            if let Err(err) = app.publish_pending_work_state() {
                return CommandResult::error(format!(
                    "Session renamed, but Work views were not published: {err}"
                ));
            }
            CommandResult::message(format!("Session renamed to \"{new_title}\""))
        }
        Err(e) => CommandResult::error(format!("Could not save session: {e}")),
    }
}

/// Recover the session document for a live turn that has not completed (and
/// therefore persisted) its first snapshot yet (#5430).
///
/// Until a turn completes, `sessions/<id>.json` does not exist — only the
/// crash checkpoint written at dispatch does — so a mid-first-turn
/// `/rename` or `/title` used to fail outright with "Could not load
/// session". Prefer the durable checkpoint; if even that has not been
/// flushed yet, rebuild the document from the same in-memory `App` state the
/// checkpoint itself was built from. Everything rename/title needs to
/// preserve (id, created_at, fork lineage, journal) is either in the
/// checkpoint or has never existed, and `update_session` re-syncs the
/// conversation from `App` state immediately afterwards in both cases.
pub(crate) fn live_session_before_first_snapshot(
    manager: &SessionManager,
    session_id: &str,
    app: &App,
) -> Option<crate::session_manager::SavedSession> {
    if let Ok(Some(checkpoint)) = manager.load_session_checkpoint(session_id) {
        return Some(checkpoint);
    }
    Some(
        crate::session_manager::create_saved_session_with_id_and_mode(
            session_id.to_string(),
            &app.api_messages,
            &app.model_selection_for_persistence(),
            &app.workspace,
            u64::from(app.session.total_tokens),
            app.system_prompt.as_ref(),
            Some(app.mode.as_setting()),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::Role;
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
    fn rename_without_arg_returns_error() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        let r = rename(&mut app, None);
        assert!(r.is_error);
        assert!(r.message.unwrap().contains("Usage:"));
    }

    #[test]
    fn rename_with_empty_arg_returns_error() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        let r = rename(&mut app, Some("   "));
        assert!(r.is_error);
        assert!(r.message.unwrap().contains("Usage:"));
    }

    #[test]
    fn rename_without_active_session_returns_error() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        app.current_session_id = None;
        let r = rename(&mut app, Some("My Session"));
        assert!(r.is_error);
        assert!(r.message.unwrap().contains("No active session"));
    }

    #[test]
    fn rename_title_too_long_returns_error() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app(&tmp);
        let long_title = "a".repeat(MAX_TITLE_LEN + 1);
        let r = rename(&mut app, Some(&long_title));
        assert!(r.is_error);
        assert!(r.message.unwrap().contains("too long"));
    }

    #[test]
    fn rename_persists_new_title() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);

        let stale_prompt = crate::models::SystemPrompt::Text("stale prompt".to_string());
        let session = create_saved_session_with_mode(
            &[],
            "deepseek-v4-pro",
            tmp.path(),
            0,
            Some(&stale_prompt),
            None,
        );
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).unwrap();
        app.set_model_selection("local-code-model".to_string());
        app.set_provider_identity(crate::config::ApiProvider::Custom, "lm-studio");
        app.mode = crate::tui::app::AppMode::Operate;
        app.system_prompt = None;
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add(
                "live rename state".to_string(),
                crate::tools::todo::TodoStatus::InProgress,
            );
        }
        let expected_work_state = app.work_state_snapshot().expect("work snapshot");

        let result = rename_with_manager("Brand New Title", &session_id, &manager, &mut app);
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("Brand New Title"));

        let reloaded = manager.load_session(&session_id).unwrap();
        assert_eq!(reloaded.metadata.title, "Brand New Title");
        assert_eq!(reloaded.work_state, expected_work_state);
        assert!(reloaded.system_prompt.is_none());
        assert_eq!(reloaded.metadata.model, "local-code-model");
        assert_eq!(reloaded.metadata.model_provider, "custom");
        assert_eq!(
            reloaded.metadata.model_provider_id.as_deref(),
            Some("lm-studio")
        );
        assert_eq!(reloaded.metadata.workspace, app.workspace);
        assert_eq!(reloaded.metadata.mode.as_deref(), Some("operate"));
        assert_eq!(app.session_title.as_deref(), Some("Brand New Title"));
        assert_eq!(
            app.current_session_metadata
                .as_ref()
                .map(|metadata| metadata.title.as_str()),
            Some("Brand New Title")
        );
    }

    #[test]
    fn rename_strips_terminal_controls_before_persisting() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);
        let session =
            create_saved_session_with_mode(&[], "deepseek-v4-pro", tmp.path(), 0, None, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).unwrap();
        app.current_session_id = Some(session_id.clone());

        let result = rename_with_manager(
            "Ev\u{1b}]0;PWNED\u{7}il\u{202e} Beta",
            &session_id,
            &manager,
            &mut app,
        );
        assert!(!result.is_error, "{result:?}");
        let reloaded = manager.load_session(&session_id).unwrap();
        assert_eq!(reloaded.metadata.title, "Ev]0;PWNEDil Beta");
        assert_eq!(app.session_title.as_deref(), Some("Ev]0;PWNEDil Beta"));

        // Controls alone are the same as no title at all.
        let result = rename_with_manager("\u{1b}\u{7}\u{200b}", &session_id, &manager, &mut app);
        assert!(result.is_error);
    }

    #[test]
    fn rename_title_at_max_length_succeeds() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);

        let session =
            create_saved_session_with_mode(&[], "deepseek-v4-pro", tmp.path(), 0, None, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).unwrap();

        let max_title = "中".repeat(MAX_TITLE_LEN);
        let result = rename_with_manager(&max_title, &session_id, &manager, &mut app);
        assert!(!result.is_error);

        let reloaded = manager.load_session(&session_id).unwrap();
        assert_eq!(reloaded.metadata.title, max_title);
    }

    // #5430: until a session's first turn completes, only the crash
    // checkpoint written at dispatch exists — `sessions/<id>.json` does not.
    // A mid-first-turn `/rename`/`/title` must apply from that checkpoint
    // instead of failing with "Could not load session".
    #[test]
    fn rename_mid_first_turn_recovers_from_checkpoint_when_snapshot_missing() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);

        let checkpoint =
            create_saved_session_with_mode(&[], "deepseek-v4-pro", tmp.path(), 0, None, None);
        let session_id = checkpoint.metadata.id.clone();
        manager.save_checkpoint(&checkpoint).unwrap();
        app.current_session_id = Some(session_id.clone());
        app.api_messages = vec![user_message("first turn still streaming")];

        let result = rename_with_manager("Midturn Rename", &session_id, &manager, &mut app);
        assert!(!result.is_error, "{result:?}");
        assert_eq!(app.session_title.as_deref(), Some("Midturn Rename"));

        // The rename promotes the checkpoint record to the durable session
        // file, so the listing and the next launch see the new title.
        let persisted = manager.load_session(&session_id).unwrap();
        assert_eq!(persisted.metadata.title, "Midturn Rename");
        assert_eq!(persisted.messages.len(), 1);
    }

    // Worst case of the same window: the user renames before even the
    // dispatch checkpoint has been flushed. The document is then built from
    // the same in-memory App state the checkpoint itself would carry.
    #[test]
    fn rename_mid_first_turn_builds_from_app_state_when_nothing_persisted() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let mut app = make_app(&tmp);

        let session_id = "live-before-first-checkpoint";
        app.current_session_id = Some(session_id.to_string());
        app.api_messages = vec![user_message("turn one, nothing persisted yet")];

        let result = rename_with_manager("Earliest Rename", session_id, &manager, &mut app);
        assert!(!result.is_error, "{result:?}");
        assert_eq!(app.session_title.as_deref(), Some("Earliest Rename"));

        let persisted = manager.load_session(session_id).unwrap();
        assert_eq!(persisted.metadata.title, "Earliest Rename");
        assert_eq!(persisted.messages.len(), 1);
    }

    fn user_message(text: &str) -> crate::models::Message {
        crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }
}
