//! Skills command area: listing and running skills, review, and restore.

mod restore;
mod review;
// This group dir intentionally has a `skills.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
mod skills;

pub(in crate::commands) use self::skills::run_skill_by_name;

use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct SkillsCommands;

impl CommandGroup for SkillsCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                skills::SkillsCmd::info(),
                skills::SkillsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                skills::SkillCmd::info(),
                skills::SkillCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                review::ReviewCmd::info(),
                review::ReviewCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                restore::RestoreCmd::info(),
                restore::RestoreCmd::execute,
            )),
        ])
    }
}
