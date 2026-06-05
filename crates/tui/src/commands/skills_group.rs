//! Skills commands group — skills, skill, review, restore

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Skills;
impl Command for Skills {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "skills", aliases: &["jinengliebiao"], usage: "/skills [--remote|sync|<prefix>]", description_id: MessageId::CmdSkillsDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::skills::list_skills(app, args) }
}

pub struct Skill;
impl Command for Skill {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "skill", aliases: &["jineng"], usage: "/skill <name|install <spec>|update <name>|uninstall <name>|trust <name>>", description_id: MessageId::CmdSkillDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::skills::run_skill(app, args) }
}

pub struct Review;
impl Command for Review {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "review", aliases: &["shencha"], usage: "/review <target>", description_id: MessageId::CmdReviewDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::review::review(app, args) }
}

pub struct Restore;
impl Command for Restore {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "restore", aliases: &[], usage: "/restore [N]", description_id: MessageId::CmdRestoreDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::restore::restore(app, args) }
}

pub struct SkillsCommands;
impl CommandGroup for SkillsCommands {
    fn group_name(&self) -> &'static str { "Skills" }
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Skills),
            Box::new(Skill),
            Box::new(Review),
            Box::new(Restore),
        ]
    }
}
