//! Cache command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::cache_impl::cache(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Cache.info();
        assert_eq!(info.name, "cache");
        assert!(!info.usage.is_empty());
    }
}
