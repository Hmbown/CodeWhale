//! Slop command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;
use super::slop_impl::slop;

pub struct Slop;
impl Command for Slop {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "slop",
            aliases: &["canzha"],
            usage: "/slop [query|export]",
            description_id: MessageId::CmdSlopDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        slop(app, args)
    }
}
