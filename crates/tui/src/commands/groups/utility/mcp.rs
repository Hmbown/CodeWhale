//! Mcp command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Mcp;
impl Command for Mcp {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "mcp",
            aliases: &[],
            usage: "/mcp [list|restart|stop|start|add|remove]",
            description_id: MessageId::CmdMcpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::mcp::mcp(app, args)
    }
}
