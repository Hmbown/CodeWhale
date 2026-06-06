//! Trust command.

use super::trust_impl::trust;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Trust;
impl Command for Trust {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "trust",
            aliases: &["xinren"],
            usage: "/trust [path]",
            description_id: MessageId::CmdTrustDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        trust(app, args)
    }
}
