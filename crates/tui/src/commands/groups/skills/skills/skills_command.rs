//! Skills command.

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandInfo};
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
        super::skills_impl::list_skills(app, args)
    }
}
#[cfg(test)]
mod command_metadata_tests {
    use super::*;

    #[test]
    fn info_returns_metadata() {
        let info = Skills.info();
        assert_eq!(info.name, "skills");
        assert!(!info.usage.is_empty());
    }
}
