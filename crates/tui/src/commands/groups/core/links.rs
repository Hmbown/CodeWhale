//! `/links` command.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "links",
    aliases: &["dashboard", "api", "lianjie"],
    usage: "/links",
    description_id: MessageId::CmdLinksDescription,
};

pub(in crate::commands) struct LinksCmd;

impl RegisterCommand for LinksCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        super::core::deepseek_links(app)
    }
}
