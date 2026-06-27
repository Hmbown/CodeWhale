use crate::commands::CommandResult;
use crate::commands::traits::{
    Command, CommandGroup, CommandInfo, FunctionCommand, RegisterCommand,
};
use crate::localization::MessageId;
use crate::plugins;
use crate::tui::app::App;

pub struct PluginsCommands;

impl CommandGroup for PluginsCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![Box::new(FunctionCommand::new(
            PluginsCmd::info(),
            PluginsCmd::execute,
        ))]
    }
}

pub(in crate::commands) const PLUGINS_INFO: CommandInfo = CommandInfo {
    name: "plugins",
    aliases: &["plugin"],
    usage: "/plugins [list|enable|disable|info] [name]",
    description_id: MessageId::CmdPluginDescription,
};

pub(in crate::commands) struct PluginsCmd;

impl RegisterCommand for PluginsCmd {
    fn info() -> &'static CommandInfo {
        &PLUGINS_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        plugin_command(app, arg)
    }
}

pub fn plugin_command(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let parts: Vec<&str> = arg
        .unwrap_or("")
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .collect();

    if parts.is_empty() || parts[0] == "list" {
        return cmd_list();
    }

    match parts[0] {
        "list" => cmd_list(),
        "enable" => {
            if parts.len() < 2 {
                return CommandResult::error("Usage: /plugin enable <name>".to_string());
            }
            cmd_enable(parts[1])
        }
        "disable" => {
            if parts.len() < 2 {
                return CommandResult::error("Usage: /plugin disable <name>".to_string());
            }
            cmd_disable(parts[1])
        }
        "info" => {
            if parts.len() < 2 {
                return CommandResult::error("Usage: /plugin info <name>".to_string());
            }
            cmd_info(parts[1])
        }
        _ => {
            let name = parts.join(" ");
            show_plugin_detail(name)
        }
    }
}

fn cmd_list() -> CommandResult {
    let summary = plugins::with_registry(|r| {
        let list = r.list();
        if list.is_empty() {
            return "No plugins found.".to_string();
        }
        let mut out = String::new();
        out.push_str(&format!("Plugins ({}):\n", list.len()));
        for p in &list {
            let status = if p.enabled { "✅" } else { "⬜" };
            let source = if p.source == "builtin" { "📦" } else { "👤" };
            let skills_info = if p.skill_count > 0 {
                format!(" ({} skills)", p.skill_count)
            } else {
                String::new()
            };
            out.push_str(&format!("  {status} {source} **{}**{} — {}\n", p.name, skills_info, p.description));
        }
        out
    });
    CommandResult::message(summary)
}

fn cmd_enable(name: &str) -> CommandResult {
    let result = plugins::with_registry_mut(|r| r.enable(name));
    match result {
        Ok(()) => CommandResult::message(format!("Plugin '{name}' enabled ✅")),
        Err(e) => CommandResult::error(e),
    }
}

fn cmd_disable(name: &str) -> CommandResult {
    let result = plugins::with_registry_mut(|r| r.disable(name));
    match result {
        Ok(()) => CommandResult::message(format!("Plugin '{name}' disabled ⬜")),
        Err(e) => CommandResult::error(e),
    }
}

fn cmd_info(name: &str) -> CommandResult {
    plugins::with_registry(|r| {
        let list = r.list();
        let plugin = list.into_iter().find(|s| s.name == name);
        match plugin {
            Some(p) => {
                let status = if p.enabled { "✅ enabled" } else { "⬜ disabled" };
                let mut out = format!(
                    "**{}** — {}\n- Status: {}\n- Source: {}\n",
                    p.name, p.description, status, p.source
                );
                if let Some(ref v) = p.version {
                    out.push_str(&format!("- Version: {v}\n"));
                }
                if p.skill_count > 0 {
                    out.push_str(&format!("- Skills: {} skill(s)\n", p.skill_count));
                }
                CommandResult::message(out)
            }
            None => CommandResult::error(format!("Plugin '{name}' not found")),
        }
    })
}

fn show_plugin_detail(name: &str) -> CommandResult {
    plugins::with_registry(|r| {
        let list = r.list();
        let plugin = list.into_iter().find(|s| s.name == name);
        match plugin {
            Some(p) => {
                let status = if p.enabled { "✅ enabled" } else { "⬜ disabled" };
                let mut out = format!(
                    "**{}** — {}\n- Status: {}\n- Source: {}\n",
                    p.name, p.description, status, p.source
                );
                if let Some(ref v) = p.version {
                    out.push_str(&format!("- Version: {v}\n"));
                }
                if p.skill_count > 0 {
                    out.push_str(&format!("- Skills: {} skill(s)\n", p.skill_count));
                }
                CommandResult::message(out)
            }
            None => CommandResult::error(format!("Plugin '{name}' not found")),
        }
    })
}
