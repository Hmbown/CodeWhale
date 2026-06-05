//! Settings command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;
use super::settings_impl::show_settings;

pub struct Settings;
impl Command for Settings {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "settings",
            aliases: &[],
            usage: "/settings",
            description_id: MessageId::CmdSettingsDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        show_settings(app)
    }
}
