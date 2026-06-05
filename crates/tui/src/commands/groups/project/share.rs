//! Share command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Share;
impl Command for Share {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "share",
            aliases: &[],
            usage: "/share [path]",
            description_id: MessageId::CmdShareDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::share::share(app, args)
    }
}
