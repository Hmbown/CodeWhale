//! Task commands: add/list/show/cancel

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction};

use crate::commands::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "task",
    aliases: &["tasks"],
    usage: "/task [add <prompt>|list|digest|show <id>|cancel <id>]",
    description_id: MessageId::CmdTaskDescription,
};

pub(in crate::commands) struct TaskCmd;

impl RegisterCommand for TaskCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        task(app, arg)
    }
}

fn task(app: &mut App, args: Option<&str>) -> CommandResult {
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
        "digest" => {
            let Some(work) = app.runtime_services.work.as_ref() else {
                return CommandResult::message("No active operations or to-do items.");
            };
            match work.capture(app.current_session_id.as_deref()) {
                Ok(snapshot) => CommandResult::message(crate::work_graph::format_operation_digest(
                    snapshot.as_ref(),
                )),
                Err(error) => CommandResult::error(format!(
                    "Operation digest is temporarily unavailable: {error}"
                )),
            }
        }
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
        _ => CommandResult::error("Usage: /task [add <prompt>|list|digest|show <id>|cancel <id>]"),
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
                use_alt_screen: false,
                max_subagents: 2,
                ..crate::test_support::test_tui_options(PathBuf::from("."))
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

    #[test]
    fn digest_uses_canonical_work_runtime_without_another_state_store() {
        let mut app = app();
        let result = task(&mut app, Some("digest"));
        assert_eq!(
            result.message.as_deref(),
            Some("No active operations or to-do items.")
        );
        assert!(result.action.is_none());
    }
}
