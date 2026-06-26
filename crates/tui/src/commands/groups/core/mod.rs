//! Core command area: model/provider selection, help, navigation, and the
//! persistent RLM / sub-agent entry points.

#[cfg(all(test, feature = "long-running-tests"))]
mod acceptance;
mod agent;
mod anchor;
mod clear;
// This group dir intentionally has a `core.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
mod core;
mod exit;
mod feedback;
mod fleet;
mod help;
mod hf;
mod home;
mod hooks;
mod hotbar;
mod setup;
mod links;
mod model;
mod modeldb;
mod models;
mod profile;
mod provider;
mod queue;
mod rlm;
mod stash;
mod subagents;
mod translate;
pub mod util;
pub mod voice;
mod workspace;

pub(in crate::commands) use self::core::reset_conversation_state;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, FunctionCommand, RegisterCommand};

pub struct CoreCommands;

impl CommandGroup for CoreCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(FunctionCommand::new(
                anchor::AnchorCmd::info(),
                anchor::AnchorCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                help::HelpCmd::info(),
                help::HelpCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                clear::ClearCmd::info(),
                clear::ClearCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                exit::ExitCmd::info(),
                exit::ExitCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                model::ModelCmd::info(),
                model::ModelCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                models::ModelsCmd::info(),
                models::ModelsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                modeldb::ModelDbCmd::info(),
                modeldb::ModelDbCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                provider::ProviderCmd::info(),
                provider::ProviderCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                queue::QueueCmd::info(),
                queue::QueueCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                stash::StashCmd::info(),
                stash::StashCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                hooks::HooksCmd::info(),
                hooks::HooksCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                subagents::SubagentsCmd::info(),
                subagents::SubagentsCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                fleet::FleetCmd::info(),
                fleet::FleetCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                hotbar::HotbarCmd::info(),
                hotbar::HotbarCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                setup::SetupCmd::info(),
                setup::SetupCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                agent::AgentCmd::info(),
                agent::AgentCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                links::LinksCmd::info(),
                links::LinksCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                feedback::FeedbackCmd::info(),
                feedback::FeedbackCmd::execute,
            )),
            Box::new(FunctionCommand::new(hf::HfCmd::info(), hf::HfCmd::execute)),
            Box::new(FunctionCommand::new(
                home::HomeCmd::info(),
                home::HomeCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                workspace::WorkspaceCmd::info(),
                workspace::WorkspaceCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                profile::ProfileCmd::info(),
                profile::ProfileCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                rlm::RlmCmd::info(),
                rlm::RlmCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                translate::TranslateCmd::info(),
                translate::TranslateCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceCmd::info(),
                voice::VoiceCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceSendCmd::info(),
                voice::VoiceSendCmd::execute,
            )),
            Box::new(FunctionCommand::new(
                voice::VoiceControlCmd::info(),
                voice::VoiceControlCmd::execute,
            )),
        ]
    }
}
