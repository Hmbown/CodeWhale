//! Skills command.

use crate::commands::traits::{Command, CommandInfo};
use crate::commands::CommandResult;
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct Skills;
impl Command for Skills {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "skills",
            aliases: &["jinengliebiao"],
            usage: "/skills [--remote|sync|<prefix>]",
            description_id: MessageId::CmdSkillsDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        crate::commands::back::skills::list_skills(app, args)
    }
}
