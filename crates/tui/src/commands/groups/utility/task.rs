//! Task commands: add/list/show/cancel

use crate::tui::app::{App, AppAction};

use super::CommandResult;

pub fn task(_app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("list") {
        return CommandResult::action(AppAction::TaskList);
    }

    let mut parts = raw.splitn(2, char::is_whitespace);
    let action = parts.next().unwrap_or("").to_ascii_lowercase();
    let remainder = parts.next().map(str::trim).filter(|s| !s.is_empty());

    match action.as_str() {
        "add" => {
            let Some(prompt) = remainder else {
                return CommandResult::error("Usage: /task add <prompt>");
            };
            CommandResult::action(AppAction::TaskAdd {
                prompt: prompt.to_string(),
            })
        }
        "list" => CommandResult::action(AppAction::TaskList),
        "show" => {
            let Some(id) = remainder else {
                return CommandResult::error("Usage: /task show <id>");
            };
            CommandResult::action(AppAction::TaskShow { id: id.to_string() })
        }
        "cancel" | "stop" => {
            let Some(id) = remainder else {
                return CommandResult::error("Usage: /task cancel <id>");
            };
            CommandResult::action(AppAction::TaskCancel { id: id.to_string() })
        }
        _ => CommandResult::error("Usage: /task [add <prompt>|list|show <id>|cancel <id>]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-pro".to_string(),
                workspace: PathBuf::from("."),
                config_path: None,
                config_profile: None,
                allow_shell: false,
                use_alt_screen: false,
                use_mouse_capture: false,
                use_bracketed_paste: true,
                max_subagents: 2,
                skills_dir: PathBuf::from("."),
                memory_path: PathBuf::from("memory.md"),
                notes_path: PathBuf::from("notes.txt"),
                mcp_config_path: PathBuf::from("mcp.json"),
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

    #[test]
    fn parses_add_and_cancel() {
        let mut app = app();
        let add = task(&mut app, Some("add write tests"));
        assert!(matches!(
            add.action,
            Some(AppAction::TaskAdd { prompt }) if prompt == "write tests"
        ));

        let cancel = task(&mut app, Some("cancel task_1234"));
        assert!(matches!(
            cancel.action,
            Some(AppAction::TaskCancel { id }) if id == "task_1234"
        ));
    }

    #[test]
    fn validates_usage() {
        let mut app = app();
        let result = task(&mut app, Some("add"));
        assert!(result.message.is_some());
        assert!(result.action.is_none());
    }
}
