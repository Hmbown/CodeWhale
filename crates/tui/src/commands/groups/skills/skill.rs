//! Skill command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Skill;
impl Command for Skill {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "skill",
            aliases: &["jineng"],
            usage: "/skill <name|install|update|uninstall|trust>",
            description_id: MessageId::CmdSkillDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::skills::run_skill(app, args)
    }
}
