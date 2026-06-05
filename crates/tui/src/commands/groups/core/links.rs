//! Links command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Links;
impl Command for Links {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "links",
            aliases: &["dashboard", "api", "lianjie"],
            usage: "/links",
            description_id: MessageId::CmdLinksDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        crate::commands::back::core::deepseek_links(app)
    }
}
