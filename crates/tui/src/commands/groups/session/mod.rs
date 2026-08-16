//! Session command area: saving, forking, resuming, exporting, and the
//! `/relay` session-handoff artifact.

#[cfg(all(test, feature = "long-running-tests"))]
mod acceptance;
mod branch;
mod compact;
mod export;
mod fork;
mod load;
mod new;
mod purge;
mod relay;
mod remote_control;
mod remote_env;
mod rename;
#[cfg(test)]
pub(crate) use rename::rename_with_manager;
mod resume;
mod save;
mod sessions;
mod structcopy;
mod title;
mod tree;
// This group dir intentionally has a `session.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
mod session;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct SessionCommands;

impl CommandGroup for SessionCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(
                rename::RenameCmd::info(),
                rename::RenameCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                title::TitleCmd::info(),
                title::TitleCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                save::SaveCmd::info(),
                save::SaveCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                fork::ForkCmd::info(),
                fork::ForkCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                new::NewCmd::info(),
                new::NewCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                sessions::SessionsCmd::info(),
                sessions::SessionsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                load::LoadCmd::info(),
                load::LoadCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                resume::ResumeCmd::info(),
                resume::ResumeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                tree::TreeCmd::info(),
                tree::TreeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                branch::BranchCmd::info(),
                branch::BranchCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                compact::CompactCmd::info(),
                compact::CompactCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                purge::PurgeCmd::info(),
                purge::PurgeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                relay::RelayCmd::info(),
                relay::RelayCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                remote_control::RemoteControlCmd::info(),
                remote_control::RemoteControlCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                remote_env::RemoteEnvCmd::info(),
                remote_env::RemoteEnvCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                export::ExportCmd::info(),
                export::ExportCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                structcopy::StructcopyCmd::info(),
                structcopy::StructcopyCmd::execute,
            )),
        ])
    }
}
