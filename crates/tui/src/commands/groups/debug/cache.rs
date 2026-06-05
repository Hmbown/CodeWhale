//! Cache command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Cache;
impl Command for Cache {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "cache",
            aliases: &[],
            usage: "/cache [count|inspect|stats|zones|warmup]",
            description_id: MessageId::CmdCacheDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::shared::debug::cache(app, args)
    }
}
