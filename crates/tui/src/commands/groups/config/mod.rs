//! Config command area: settings, modes, themes, trust, and status surfaces.

// This group dir intentionally has a `config.rs` child module with the same
// name. The module_inception allow is a permanent structure rationale, not
// migration scaffolding; see docs/architecture/command-dispatch.md.
#[allow(clippy::module_inception)]
pub mod config;
mod status;

use crate::commands::CommandResult;
use crate::commands::traits::{Command, CommandGroup, CommandInfo, FunctionCommand};
use crate::localization::MessageId;
use crate::tui::app::App;

pub struct ConfigCommands;

impl CommandGroup for ConfigCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![
            Box::new(FunctionCommand::new(&CONFIG_INFO, run_config)),
            Box::new(FunctionCommand::new(&SIDEBAR_INFO, run_sidebar)),
            Box::new(FunctionCommand::new(&SETTINGS_INFO, run_settings)),
            Box::new(FunctionCommand::new(&STATUS_INFO, run_status)),
            Box::new(FunctionCommand::new(&STATUSLINE_INFO, run_statusline)),
            Box::new(FunctionCommand::new(&MODE_INFO, run_mode)),
            Box::new(FunctionCommand::new(&THEME_INFO, run_theme)),
            Box::new(FunctionCommand::new(&VERBOSE_INFO, run_verbose)),
            Box::new(FunctionCommand::new(&TRUST_INFO, run_trust)),
            Box::new(FunctionCommand::new(&LOGOUT_INFO, run_logout)),
            Box::new(FunctionCommand::new(&DEBT_INFO, run_debt)),
        ])
    }
}

static CONFIG_INFO: CommandInfo = CommandInfo {
    name: "config",
    // /experiments is a discoverable entry to the same view: the Experimental
    // section exposes the Workflow, goal, and sub-agent opt-ins (#3182).
    aliases: &["experiments", "experimental"],
    usage: "/config [ask-rules|status|<key> [value]]",
    description_id: MessageId::CmdConfigDescription,
};
static SIDEBAR_INFO: CommandInfo = CommandInfo {
    name: "sidebar",
    aliases: &[],
    usage: "/sidebar [on|off|auto|work|activity|tasks|agents|context] [--save]",
    description_id: MessageId::CmdSidebarDescription,
};
static SETTINGS_INFO: CommandInfo = CommandInfo {
    name: "settings",
    aliases: &[],
    usage: "/settings",
    description_id: MessageId::CmdSettingsDescription,
};
static STATUS_INFO: CommandInfo = CommandInfo {
    name: "status",
    aliases: &[],
    usage: "/status",
    description_id: MessageId::CmdStatusDescription,
};
static STATUSLINE_INFO: CommandInfo = CommandInfo {
    name: "statusline",
    aliases: &[],
    usage: "/statusline",
    description_id: MessageId::CmdStatuslineDescription,
};
static MODE_INFO: CommandInfo = CommandInfo {
    name: "mode",
    aliases: &["jihua", "zidong"],
    usage: "/mode [act|plan|operate|1|2|3]",
    description_id: MessageId::CmdModeDescription,
};
static THEME_INFO: CommandInfo = CommandInfo {
    name: "theme",
    aliases: &[],
    usage: "/theme [name]",
    description_id: MessageId::CmdThemeDescription,
};
static VERBOSE_INFO: CommandInfo = CommandInfo {
    name: "verbose",
    aliases: &[],
    usage: "/verbose [on|off]",
    description_id: MessageId::CmdVerboseDescription,
};
static TRUST_INFO: CommandInfo = CommandInfo {
    name: "trust",
    aliases: &["xinren"],
    usage: "/trust [on|off|add <path>|remove <path>|list]",
    description_id: MessageId::CmdTrustDescription,
};
static LOGOUT_INFO: CommandInfo = CommandInfo {
    name: "logout",
    aliases: &[],
    usage: "/logout",
    description_id: MessageId::CmdLogoutDescription,
};
static DEBT_INFO: CommandInfo = CommandInfo {
    name: "debt",
    aliases: &["cleanup"],
    usage: "/debt [query|export]",
    description_id: MessageId::CmdSlopDescription,
};

fn run_registered(app: &mut App, name: &str, arg: Option<&str>) -> CommandResult {
    dispatch(app, name, arg).expect("registered config command should dispatch")
}

fn run_config(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "config", arg)
}
fn run_sidebar(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "sidebar", arg)
}
fn run_settings(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "settings", arg)
}
fn run_status(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "status", arg)
}
fn run_statusline(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "statusline", arg)
}
fn run_mode(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "mode", arg)
}
fn run_theme(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "theme", arg)
}
fn run_verbose(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "verbose", arg)
}
fn run_trust(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "trust", arg)
}
fn run_logout(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "logout", arg)
}
fn run_debt(app: &mut App, arg: Option<&str>) -> CommandResult {
    run_registered(app, "debt", arg)
}

pub(in crate::commands) fn dispatch(
    app: &mut App,
    command: &str,
    arg: Option<&str>,
) -> Option<CommandResult> {
    let result = match command {
        "config" | "experiments" | "experimental" => config::config_command(app, arg),
        "sidebar" => config::sidebar(app, arg),
        "settings" => config::show_settings(app),
        "status" => status::status(app),
        "statusline" => config::status_line(app),
        "mode" => config::mode(app, arg),
        "jihua" => config::mode(app, Some("plan")),
        "zidong" => config::mode(app, Some("yolo")),
        "theme" => config::theme(app, arg),
        "verbose" => config::verbose(app, arg),
        "trust" | "xinren" => config::trust(app, arg),
        "logout" => config::logout(app),
        "debt" | "cleanup" | "slop" | "canzha" => config::slop(app, arg),
        _ => return None,
    };
    Some(result)
}
