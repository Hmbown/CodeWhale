//! Skill command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::skill_impl::run_skill(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Skill.info();
        assert_eq!(info.name, "skill");
        assert!(!info.usage.is_empty());
    }
}
