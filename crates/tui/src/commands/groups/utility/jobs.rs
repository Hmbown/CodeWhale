//! Shell job-center commands.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction, ShellJobAction};

use crate::commands::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "jobs",
    aliases: &["job", "zuoye"],
    usage: "/jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|cancel <id>]",
    description_id: MessageId::CmdJobsDescription,
};

pub(in crate::commands) struct JobsCmd;

impl RegisterCommand for JobsCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        jobs(app, arg)
    }
}

fn jobs(_app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("list") {
        return CommandResult::action(AppAction::ShellJob(ShellJobAction::List));
    }

    let mut parts = raw.splitn(3, char::is_whitespace);
    let action = parts.next().unwrap_or("").to_ascii_lowercase();
    let id = parts.next().map(str::trim).filter(|s| !s.is_empty());
    let rest = parts.next().map(str::trim).unwrap_or("");

    match action.as_str() {
        "list" => CommandResult::action(AppAction::ShellJob(ShellJobAction::List)),
        "show" | "inspect" => match id {
            Some(id) => CommandResult::action(AppAction::ShellJob(ShellJobAction::Show {
                id: id.to_string(),
            })),
            None => CommandResult::error("Usage: /jobs show <id>"),
        },
        "poll" | "wait" => match id {
            Some(id) => CommandResult::action(AppAction::ShellJob(ShellJobAction::Poll {
                id: id.to_string(),
                wait: action == "wait",
            })),
            None => CommandResult::error("Usage: /jobs poll <id>"),
        },
        "stdin" | "send" => match id {
            Some(id) if !rest.is_empty() => {
                CommandResult::action(AppAction::ShellJob(ShellJobAction::SendStdin {
                    id: id.to_string(),
                    input: rest.to_string(),
                    close: false,
                }))
            }
            _ => CommandResult::error("Usage: /jobs stdin <id> <input>"),
        },
        "close-stdin" | "eof" => match id {
            Some(id) => CommandResult::action(AppAction::ShellJob(ShellJobAction::SendStdin {
                id: id.to_string(),
                input: String::new(),
                close: true,
            })),
            None => CommandResult::error("Usage: /jobs close-stdin <id>"),
        },
        "cancel" | "kill" | "stop" => match id {
            Some(id) => CommandResult::action(AppAction::ShellJob(ShellJobAction::Cancel {
                id: id.to_string(),
            })),
            None => CommandResult::error("Usage: /jobs cancel <id>"),
        },
        "cancel-all" | "kill-all" | "stop-all" => {
            CommandResult::action(AppAction::ShellJob(ShellJobAction::CancelAll))
        }
        _ => CommandResult::error(
            "Usage: /jobs [list|show <id>|poll <id>|wait <id>|stdin <id> <input>|close-stdin <id>|cancel <id>|cancel-all]",
        ),
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
    fn parses_job_actions() {
        let mut app = app();
        let show = jobs(&mut app, Some("show shell_abcd"));
        assert!(matches!(
            show.action,
            Some(AppAction::ShellJob(ShellJobAction::Show { id })) if id == "shell_abcd"
        ));

        let send = jobs(&mut app, Some("stdin shell_abcd y"));
        assert!(matches!(
            send.action,
            Some(AppAction::ShellJob(ShellJobAction::SendStdin { id, input, close: false }))
                if id == "shell_abcd" && input == "y"
        ));

        let cancel_all = jobs(&mut app, Some("cancel-all"));
        assert!(matches!(
            cancel_all.action,
            Some(AppAction::ShellJob(ShellJobAction::CancelAll))
        ));
    }
}
