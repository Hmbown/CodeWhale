//! `/rename` command — set a custom title for the current session.

use crate::session_manager::{SessionManager, update_session};
use crate::tui::app::App;

use super::CommandResult;

const MAX_TITLE_LEN: usize = 100;

/// Rename the current session to the given title.
///
/// Usage: `/rename <new title>`
///
/// The new title is persisted immediately to `~/.deepseek/sessions/<id>.json`
/// so the updated name is visible the next time the session picker is opened.
pub fn rename(app: &mut App, arg: Option<&str>) -> CommandResult {
    let new_title = match arg.map(str::trim).filter(|s| !s.is_empty()) {
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

fn rename_with_manager(
    new_title: &str,
    session_id: &str,
    manager: &SessionManager,
    app: &App,
) -> CommandResult {
    let mut session = match manager.load_session(session_id) {
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
    app.sync_cost_to_metadata(&mut session.metadata);
    session.metadata.title = new_title.to_string();

    match manager.save_session(&session) {
        Ok(_) => CommandResult::message(format!("Session renamed to \"{new_title}\"")),
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
        let app = make_app(&tmp);

        let session =
            create_saved_session_with_mode(&[], "deepseek-v4-pro", tmp.path(), 0, None, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).unwrap();

        let result = rename_with_manager("Brand New Title", &session_id, &manager, &app);
        assert!(!result.is_error);
        assert!(result.message.unwrap().contains("Brand New Title"));

        let reloaded = manager.load_session(&session_id).unwrap();
        assert_eq!(reloaded.metadata.title, "Brand New Title");
    }

    #[test]
    fn rename_title_at_max_length_succeeds() {
        let tmp = TempDir::new().unwrap();
        let manager = make_session_manager(&tmp);
        let app = make_app(&tmp);

        let session =
            create_saved_session_with_mode(&[], "deepseek-v4-pro", tmp.path(), 0, None, None);
        let session_id = session.metadata.id.clone();
        manager.save_session(&session).unwrap();

        let max_title = "中".repeat(MAX_TITLE_LEN);
        let result = rename_with_manager(&max_title, &session_id, &manager, &app);
        assert!(!result.is_error);

        let reloaded = manager.load_session(&session_id).unwrap();
        assert_eq!(reloaded.metadata.title, max_title);
    }
}
