//! Core commands group — help, clear, exit, model, models, provider, links,
//! workspace, home/stats, profile, subagents, agent, relay, feedback

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

// ---------------------------------------------------------------------------
// Help
// ---------------------------------------------------------------------------

pub struct Help;
impl Command for Help {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "help",
            aliases: &["?", "bangzhu", "帮助"],
            usage: "/help [command]",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::core::help(app, args)
    }
}

// ---------------------------------------------------------------------------
// Clear
// ---------------------------------------------------------------------------

pub struct Clear;
impl Command for Clear {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "clear",
            aliases: &["qingping"],
            usage: "/clear",
            description_id: MessageId::CmdClearDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::clear(app)
    }
}

// ---------------------------------------------------------------------------
// Exit
// ---------------------------------------------------------------------------

pub struct Exit;
impl Command for Exit {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "exit",
            aliases: &["quit", "q", "tuichu"],
            usage: "/exit",
            description_id: MessageId::CmdExitDescription,
        }
    }
    fn execute(&self, _app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::exit()
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct Model;
impl Command for Model {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "model",
            aliases: &["moxing"],
            usage: "/model [name]",
            description_id: MessageId::CmdModelDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::core::model(app, args)
    }
}

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

pub struct Models;
impl Command for Models {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "models",
            aliases: &["moxingliebiao"],
            usage: "/models",
            description_id: MessageId::CmdModelsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::models(app)
    }
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

pub struct Provider;
impl Command for Provider {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "provider",
            aliases: &[],
            usage: "/provider [name] [model]",
            description_id: MessageId::CmdProviderDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::provider::provider(app, args)
    }
}

// ---------------------------------------------------------------------------
// Links / Dashboard / API
// ---------------------------------------------------------------------------

pub struct Links;
impl Command for Links {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "links",
            aliases: &["dashboard", "api", "lianjie"],
            usage: "/links",
            description_id: MessageId::CmdLinksDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::deepseek_links(app)
    }
}

// ---------------------------------------------------------------------------
// Feedback
// ---------------------------------------------------------------------------

pub struct Feedback;
impl Command for Feedback {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "feedback",
            aliases: &[],
            usage: "/feedback [bug|feature|security]",
            description_id: MessageId::CmdFeedbackDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::feedback::feedback(app, args)
    }
}

// ---------------------------------------------------------------------------
// Home / Stats / Overview
// ---------------------------------------------------------------------------

pub struct Home;
impl Command for Home {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "home",
            aliases: &["stats", "overview", "zhuye", "shouye"],
            usage: "/home",
            description_id: MessageId::CmdHomeDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::home_dashboard(app)
    }
}

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

pub struct Workspace;
impl Command for Workspace {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "workspace",
            aliases: &["cwd"],
            usage: "/workspace [path]",
            description_id: MessageId::CmdWorkspaceDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::core::workspace_switch(app, args)
    }
}

// ---------------------------------------------------------------------------
// Subagents
// ---------------------------------------------------------------------------

pub struct Subagents;
impl Command for Subagents {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "subagents",
            aliases: &["agents", "zhinengti"],
            usage: "/subagents",
            description_id: MessageId::CmdSubagentsDescription,
        }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult {
        super::core::subagents(app)
    }
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

pub struct Agent;
impl Command for Agent {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "agent",
            aliases: &["daili"],
            usage: "/agent [N] <task>",
            description_id: MessageId::CmdAgentDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::agent(app, args)
    }
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

pub struct Profile;
impl Command for Profile {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "profile",
            aliases: &["dangan"],
            usage: "/profile <name>",
            description_id: MessageId::CmdHelpDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::core::profile_switch(app, args)
    }
}

// ---------------------------------------------------------------------------
// Relay
// ---------------------------------------------------------------------------

pub struct Relay;
impl Command for Relay {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo {
            name: "relay",
            aliases: &["batonpass", "接力"],
            usage: "/relay [focus]",
            description_id: MessageId::CmdRelayDescription,
        }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        super::relay(app, args)
    }
}

// ---------------------------------------------------------------------------
// Group
// ---------------------------------------------------------------------------

pub struct CoreCommands;
impl CommandGroup for CoreCommands {
    fn group_name(&self) -> &'static str {
        "Core"
    }
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Help),
            Box::new(Clear),
            Box::new(Exit),
            Box::new(Model),
            Box::new(Models),
            Box::new(Provider),
            Box::new(Links),
            Box::new(Feedback),
            Box::new(Home),
            Box::new(Workspace),
            Box::new(Subagents),
            Box::new(Agent),
            Box::new(Profile),
            Box::new(Relay),
        ]
    }
}
