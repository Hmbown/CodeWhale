//! Skills command area: listing and running skills, review, and restore.

mod restore;
mod review;
// This group dir intentionally has a `skills.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
mod skills;

pub(in crate::commands) use self::skills::run_skill_by_name;

use crate::commands::traits::{Command, CommandGroup, ContextualCommand};

pub struct SkillsCommands;

impl CommandGroup for SkillsCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(
                ContextualCommand::from_contract::<skills::SkillsCmd>()
                    .expect("skills registration")
            ),
            Box::new(
                ContextualCommand::from_contract::<skills::SkillCmd>().expect("skill registration")
            ),
            Box::new(
                ContextualCommand::from_contract::<review::ReviewCmd>()
                    .expect("review registration")
            ),
            Box::new(
                ContextualCommand::from_contract::<restore::RestoreCmd>()
                    .expect("restore registration")
            ),
        ])
    }
}
