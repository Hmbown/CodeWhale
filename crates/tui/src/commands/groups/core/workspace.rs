//! Workspace command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Workspace;
impl Command for Workspace {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "workspace",
            aliases: &["cwd"],
            usage: "/workspace [path]",
            description_id: MessageId::CmdWorkspaceDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::core::workspace_switch(app, args)
    }
}
