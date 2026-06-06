//! Mcp command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        crate::commands::groups::utility::mcp::mcp(app, args)
    }
}

#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Mcp.info();
        assert_eq!(info.name, "mcp");
        assert!(!info.usage.is_empty());
    }
}
