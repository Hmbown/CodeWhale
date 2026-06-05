//! Config commands group — config, settings, status, statusline, mode, theme,
//! verbose, trust, logout

use crate::tui::app::App;

use super::traits::{Command, CommandGroup, CommandInfo};
use super::CommandResult;
use crate::localization::MessageId;

pub struct Config;
impl Command for Config {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "config", aliases: &[], usage: "/config [key] [value]", description_id: MessageId::CmdConfigDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::config_command(app, args) }
}

pub struct Settings;
impl Command for Settings {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "settings", aliases: &[], usage: "/settings", description_id: MessageId::CmdSettingsDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::back::config::show_settings(app) }
}

pub struct Status;
impl Command for Status {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "status", aliases: &[], usage: "/status", description_id: MessageId::CmdStatusDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::back::status::status(app) }
}

pub struct Statusline;
impl Command for Statusline {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "statusline", aliases: &[], usage: "/statusline", description_id: MessageId::CmdStatuslineDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::back::config::status_line(app) }
}

pub struct Mode;
impl Command for Mode {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "mode", aliases: &[], usage: "/mode [plan|yolo|agent]", description_id: MessageId::CmdModeDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult {
        // The aliases /jihua and /zidong are special — they set mode directly
        // (handled by the now-removed match arms). We reuse the same dispatch.
        super::back::config::mode(app, args)
    }
}

pub struct Theme;
impl Command for Theme {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "theme", aliases: &[], usage: "/theme [name]", description_id: MessageId::CmdThemeDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::theme(app, args) }
}

pub struct Verbose;
impl Command for Verbose {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "verbose", aliases: &[], usage: "/verbose [on|off]", description_id: MessageId::CmdVerboseDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::verbose(app, args) }
}

pub struct Trust;
impl Command for Trust {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "trust", aliases: &["xinren"], usage: "/trust [path]", description_id: MessageId::CmdTrustDescription }
    }
    fn execute(&self, app: &mut App, args: Option<&str>) -> CommandResult { super::back::config::trust(app, args) }
}

pub struct Logout;
impl Command for Logout {
    fn info(&self) -> &'static CommandInfo {
        &CommandInfo { name: "logout", aliases: &[], usage: "/logout", description_id: MessageId::CmdLogoutDescription }
    }
    fn execute(&self, app: &mut App, _args: Option<&str>) -> CommandResult { super::back::config::logout(app) }
}

pub struct ConfigCommands;
impl CommandGroup for ConfigCommands {

    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Config),
            Box::new(Settings),
            Box::new(Status),
            Box::new(Statusline),
            Box::new(Mode),
            Box::new(Theme),
            Box::new(Verbose),
            Box::new(Trust),
            Box::new(Logout),
        ]
    }
}
