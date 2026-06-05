//! Provider command.

use crate::tui::app::App;
use crate::commands::traits::Command;
use crate::commands::traits::CommandInfo;
use crate::commands::CommandResult;
use crate::localization::MessageId;

pub struct Provider;
impl Command for Provider {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "provider",
            aliases: &[],
            usage: "/provider [name] [model]",
            description_id: MessageId::CmdProviderDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::provider::provider(app, args)
    }
}
