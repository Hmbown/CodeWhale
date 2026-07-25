//! In-TUI MCP manager command parser.

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::{App, AppAction, McpUiAction};

use crate::commands::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "mcp",
    aliases: &[],
    usage: "/mcp [init|import|import approve <name>|import decline <name>|recommendations|add recommended <id>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|doctor|validate|restart|reload]",
    description_id: MessageId::CmdMcpDescription,
};

pub(in crate::commands) struct McpCmd;

impl RegisterCommand for McpCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        mcp(app, arg)
    }
}

fn mcp(_app: &mut App, args: Option<&str>) -> CommandResult {
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
        "recommend" | "recommended" | "recommendations" => {
            CommandResult::message(recommended_mcp_text())
        }
        "add" => parse_add(parts.collect()),
        "enable" => match parse_name(parts.next(), "Usage: /mcp enable <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Enable { name })),
            Err(msg) => CommandResult::error(msg),
        },
        "disable" => match parse_name(parts.next(), "Usage: /mcp disable <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Disable { name })),
            Err(msg) => CommandResult::error(msg),
        },
        "remove" | "rm" => match parse_name(parts.next(), "Usage: /mcp remove <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Remove { name })),
            Err(msg) => CommandResult::error(msg),
        },
        "login" => match parse_name(parts.next(), "Usage: /mcp login <name> [--scope scope]") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Login {
                name,
                scopes: parse_scopes(parts.collect()),
            })),
            Err(msg) => CommandResult::error(msg),
        },
        "logout" => match parse_name(parts.next(), "Usage: /mcp logout <name>") {
            Ok(name) => CommandResult::action(AppAction::Mcp(McpUiAction::Logout { name })),
            Err(msg) => CommandResult::error(msg),
        },
        "import" | "marketplace" | "sources" => {
            let sub = parts.next().unwrap_or("").to_ascii_lowercase();
            match sub.as_str() {
                "" | "list" | "status" => {
                    CommandResult::action(AppAction::Mcp(McpUiAction::ImportList))
                }
                "approve" | "add" => {
                    match parse_name(parts.next(), "Usage: /mcp import approve <name>") {
                        Ok(name) => {
                            CommandResult::action(AppAction::Mcp(McpUiAction::ImportApprove {
                                name,
                            }))
                        }
                        Err(msg) => CommandResult::error(msg),
                    }
                }
                "decline" | "deny" | "reject" => {
                    match parse_name(parts.next(), "Usage: /mcp import decline <name>") {
                        Ok(name) => {
                            CommandResult::action(AppAction::Mcp(McpUiAction::ImportDecline {
                                name,
                            }))
                        }
                        Err(msg) => CommandResult::error(msg),
                    }
                }
                _ => {
                    CommandResult::error("Usage: /mcp import [list|approve <name>|decline <name>]")
                }
            }
        }
        "validate" | "doctor" => CommandResult::action(AppAction::Mcp(McpUiAction::Validate)),
        "reload" | "reconnect" | "restart" => {
            CommandResult::action(AppAction::Mcp(McpUiAction::Reload))
        }
        _ => CommandResult::error(
            "Usage: /mcp [init|import|recommendations|add recommended <id>|add stdio <name> <command> [args...]|add http <name> <url>|enable <name>|disable <name>|remove <name>|login <name>|logout <name>|doctor|validate|restart|reload]",
        ),
    }
}

fn parse_name(name: Option<&str>, usage: &str) -> Result<String, String> {
    match name {
        Some(name) if !name.trim().is_empty() => Ok(name.to_string()),
        _ => Err(usage.to_string()),
    }
}

fn parse_add(parts: Vec<&str>) -> CommandResult {
    if parts
        .first()
        .is_some_and(|part| part.eq_ignore_ascii_case("recommended"))
    {
        return match parts.as_slice() {
            [_, id] if id.eq_ignore_ascii_case("hugging-face") || id.eq_ignore_ascii_case("hf") => {
                CommandResult::action(AppAction::Mcp(McpUiAction::AddHttp {
                    name: "hugging-face".to_string(),
                    url: "https://huggingface.co/mcp".to_string(),
                    transport: None,
                }))
            }
            [_, _] => CommandResult::error(
                "Unknown recommended MCP id. Run /mcp recommendations to inspect the curated list.",
            ),
            _ => CommandResult::error("Usage: /mcp add recommended <id>"),
        };
    }
    if parts.len() < 3 {
        return CommandResult::error(
            "Usage: /mcp add stdio <name> <command> [args...] OR /mcp add http <name> <url>",
        );
    }
    match parts[0].to_ascii_lowercase().as_str() {
        "stdio" => CommandResult::action(AppAction::Mcp(McpUiAction::AddStdio {
            name: parts[1].to_string(),
            command: parts[2].to_string(),
            args: parts[3..].iter().map(|s| (*s).to_string()).collect(),
        })),
        "http" => CommandResult::action(AppAction::Mcp(McpUiAction::AddHttp {
            name: parts[1].to_string(),
            url: parts[2].to_string(),
            transport: None,
        })),
        "sse" => CommandResult::action(AppAction::Mcp(McpUiAction::AddHttp {
            name: parts[1].to_string(),
            url: parts[2].to_string(),
            transport: Some("sse".to_string()),
        })),
        _ => CommandResult::error(
            "Usage: /mcp add stdio <name> <command> [args...] OR /mcp add http <name> <url>",
        ),
    }
}

fn recommended_mcp_text() -> &'static str {
    "Recommended MCP servers (suggestions only; nothing is installed automatically)\n\
     \n\
     • hugging-face — remote Hugging Face MCP endpoint\n\
       provenance: bundled Codewhale recommendation\n\
       add explicitly: /mcp add recommended hugging-face\n\
       then inspect: /mcp doctor · reload all configured servers: /mcp restart\n\
     \n\
     External sources (~/.claude.json, .mcp.json, marketplace manifests):\n\
       /mcp import — list candidates with provenance (keyboard/mouse status)\n\
       /mcp import approve <name> — create managed connector after consent\n\
       /mcp import decline <name> — durable decline until source content changes\n\
     enabled=false is a hard block and will never import. Nothing is auto-imported."
}

fn parse_scopes(parts: Vec<&str>) -> Vec<String> {
    let mut scopes = Vec::new();
    let mut iter = parts.into_iter();
    while let Some(part) = iter.next() {
        if part == "--scope" {
            let Some(value) = iter.next() else {
                continue;
            };
            for scope in value.split(',') {
                let scope = scope.trim();
                if !scope.is_empty() {
                    scopes.push(scope.to_string());
                }
            }
            continue;
        }
        let value = part.strip_prefix("--scope=");
        let Some(value) = value else {
            for scope in part.split(',') {
                let scope = scope.trim();
                if !scope.is_empty() {
                    scopes.push(scope.to_string());
                }
            }
            continue;
        };
        for scope in value.split(',') {
            let scope = scope.trim();
            if !scope.is_empty() {
                scopes.push(scope.to_string());
            }
        }
    }
    scopes
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
                use_alt_screen: false,
                max_subagents: 2,
                ..crate::test_support::test_tui_options(PathBuf::from("."))
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

        let doctor = mcp(&mut app, Some("doctor"));
        assert!(matches!(
            doctor.action,
            Some(AppAction::Mcp(McpUiAction::Validate))
        ));
        let restart = mcp(&mut app, Some("restart"));
        assert!(matches!(
            restart.action,
            Some(AppAction::Mcp(McpUiAction::Reload))
        ));

        let recommended = mcp(&mut app, Some("recommendations"))
            .message
            .expect("recommendations text");
        assert!(recommended.contains("nothing is installed automatically"));
        assert!(recommended.contains("provenance:"));

        let add_recommended = mcp(&mut app, Some("add recommended hugging-face"));
        assert!(matches!(
            add_recommended.action,
            Some(AppAction::Mcp(McpUiAction::AddHttp { name, url, transport: None }))
                if name == "hugging-face" && url == "https://huggingface.co/mcp"
        ));

        let import_list = mcp(&mut app, Some("import"));
        assert!(matches!(
            import_list.action,
            Some(AppAction::Mcp(McpUiAction::ImportList))
        ));
        let import_approve = mcp(&mut app, Some("import approve local-tools"));
        assert!(matches!(
            import_approve.action,
            Some(AppAction::Mcp(McpUiAction::ImportApprove { name }))
                if name == "local-tools"
        ));
        let import_decline = mcp(&mut app, Some("import decline local-tools"));
        assert!(matches!(
            import_decline.action,
            Some(AppAction::Mcp(McpUiAction::ImportDecline { name }))
                if name == "local-tools"
        ));
        let marketplace = mcp(&mut app, Some("marketplace"));
        assert!(matches!(
            marketplace.action,
            Some(AppAction::Mcp(McpUiAction::ImportList))
        ));
        assert!(recommended.contains("/mcp import"));

        let login = mcp(
            &mut app,
            Some("login remote --scope tools/read,tools/write"),
        );
        assert!(matches!(
            login.action,
            Some(AppAction::Mcp(McpUiAction::Login { name, scopes }))
                if name == "remote"
                    && scopes == vec!["tools/read".to_string(), "tools/write".to_string()]
        ));
    }
}
