//! In-TUI MCP manager command parser.

use crate::tui::app::{App, AppAction, McpUiAction};

use super::CommandResult;
use crate::localization::Locale;

pub fn mcp(app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() || raw.eq_ignore_ascii_case("status") || raw.eq_ignore_ascii_case("list") {
        return CommandResult::action(AppAction::Mcp(McpUiAction::Show));
    }

    let mut parts = raw.split_whitespace();
    let action = parts.next().unwrap_or("").to_ascii_lowercase();
    match action.as_str() {
        "init" => CommandResult::action(AppAction::Mcp(McpUiAction::Init {
            force: parts.any(|part| part == "--force" || part == "-f"),
        })),
        "add" => parse_add(parts.collect(), app.ui_locale),
        "enable" => match parse_name(parts.next(), "Usage: /mcp enable <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Enable { name })),
            Err(msg) => CommandResult::error(msg, app.ui_locale),
        },
        "disable" => match parse_name(parts.next(), "Usage: /mcp disable <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Disable { name })),
            Err(msg) => CommandResult::error(msg, app.ui_locale),
        },
        "remove" | "rm" => match parse_name(parts.next(), "Usage: /mcp remove <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Remove { name })),
            Err(msg) => CommandResult::error(msg, app.ui_locale),
        },
        "validate" => CommandResult::action(AppAction::Mcp(McpUiAction::Validate)),
        "reload" | "reconnect" => CommandResult::action(AppAction::Mcp(McpUiAction::Reload)),
        _ => CommandResult::error(
            "Usage: /mcp [init|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|validate|reload]",
            app.ui_locale,
        ),
    }
}

fn parse_name(name: Option<&str>, usage: &str) -> Result<String, String> {
    match name {
        Some(name) if !name.trim().is_empty() => Ok(name.to_string()),
        _ => Err(usage.to_string()),
    }
}

fn parse_add(parts: Vec<&str>, locale: Locale) -> CommandResult {
    if parts.len() < 3 {
        return CommandResult::error(
            "Usage: /mcp add stdio <name> <command> [args...] OR /mcp add http <name> <url>",
            locale,
        );
    }
    match parts[0].to_ascii_lowercase().as_str() {
        "stdio" => CommandResult::action(AppAction::Mcp(McpUiAction::AddStdio {
            name: parts[1].to_string(),
            command: parts[2].to_string(),
            args: parts[3..].iter().map(|s| (*s).to_string()).collect(),
        })),
        "http" | "sse" => CommandResult::action(AppAction::Mcp(McpUiAction::AddHttp {
            name: parts[1].to_string(),
            url: parts[2].to_string(),
        })),
        _ => CommandResult::error(
            "Usage: /mcp add stdio <name> <command> [args...] OR /mcp add http <name> <url>",
            locale,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-pro".to_string(),
                workspace: PathBuf::from("."),
                config_path: None,
                config_profile: None,
                allow_shell: false,
                use_alt_screen: false,
                use_mouse_capture: false,
                use_bracketed_paste: true,
                max_subagents: 2,
                skills_dir: PathBuf::from("."),
                memory_path: PathBuf::from("memory.md"),
                notes_path: PathBuf::from("notes.txt"),
                mcp_config_path: PathBuf::from("mcp.json"),
                use_memory: false,
                start_in_agent_mode: false,
                skip_onboarding: true,
                yolo: false,
                resume_session_id: None,
                initial_input: None,
            },
            &Config::default(),
        )
    }

    #[test]
    fn parses_add_and_validate() {
        let mut app = app();
        let add = mcp(&mut app, Some("add stdio local node server.js"));
        assert!(matches!(
            add.action,
            Some(AppAction::Mcp(McpUiAction::AddStdio { name, command, args }))
                if name == "local" && command == "node" && args == vec!["server.js".to_string()]
        ));

        let validate = mcp(&mut app, Some("validate"));
        assert!(matches!(
            validate.action,
            Some(AppAction::Mcp(McpUiAction::Validate))
        ));
    }
}
