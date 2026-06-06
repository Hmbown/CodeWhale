//! Settings command.

use super::settings_impl::show_settings;
use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
use crate::localization::MessageId;
use crate::tui::app::App;

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
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        show_settings(app)
    }
}
