//! MCP manager formatting and UI action helpers.

use crate::localization::{Locale, MessageId, tr};
use crate::mcp::{
    McpManagerSnapshot, McpServerCapabilityMetadata, McpServerSnapshot, format_mcp_tool_description,
};
use crate::tui::app::App;
use crate::tui::history::HistoryCell;
use crate::tui::pager::PagerView;

pub(super) fn format_mcp_manager(snapshot: &McpManagerSnapshot, locale: Locale) -> String {
    let mut lines = vec![
        format!("MCP config: {}", snapshot.config_path.display()),
        format!("Config exists: {}", snapshot.config_exists),
    ];
    if snapshot.reload_required {
        lines.push(
            "Reload required: MCP config changed; run /mcp reload to rebuild the live model-visible tool pool."
                .to_string(),
        );
    } else {
        lines.push("Reload required: no pending config change.".to_string());
    }
    lines.push(String::new());

    if snapshot.servers.is_empty() {
        lines.push("No MCP servers configured.".to_string());
    } else {
        lines.push(format!("Servers ({})", snapshot.servers.len()));
        lines.push("----------------------------------------".to_string());
        for server in &snapshot.servers {
            push_server(lines.as_mut(), server, locale);
        }
    }

    lines.push(String::new());
    lines.push("Next: Connect /mcp reload · Diagnose /mcp validate".to_string());
    lines.join("\n")
}

fn push_server(lines: &mut Vec<String>, server: &McpServerSnapshot, locale: Locale) {
    let state = if server.enabled {
        if server.connected {
            "connected"
        } else if server.error.is_some() {
            "failed"
        } else {
            "enabled"
        }
    } else {
        "disabled"
    };
    let required = if server.required { " required" } else { "" };
    lines.push(format!(
        "- {} [{}{}] {} {}",
        server.name, state, required, server.transport, server.command_or_url
    ));
    lines.push(format!(
        "  timeouts: connect={}s execute={}s read={}s",
        server.connect_timeout, server.execute_timeout, server.read_timeout
    ));
    if let Some(error) = server.error.as_ref() {
        lines.push(format!("  error: {error}"));
    }
    let recovery = crate::mcp::mcp_recovery_kind(
        server.enabled,
        true,
        server.connected,
        server.error.as_deref(),
        false,
    );
    let command = if crate::mcp::mcp_name_is_command_safe(&server.name)
        || matches!(
            recovery,
            crate::mcp::McpRecoveryKind::Connect
                | crate::mcp::McpRecoveryKind::Reconnect
                | crate::mcp::McpRecoveryKind::Diagnose
        ) {
        recovery.slash_command(&server.name)
    } else {
        "/mcp validate".to_string()
    };
    let verb = match recovery {
        crate::mcp::McpRecoveryKind::Enable => "Enable",
        crate::mcp::McpRecoveryKind::Connect => "Connect",
        crate::mcp::McpRecoveryKind::Reconnect => "Reconnect",
        crate::mcp::McpRecoveryKind::Reauth => "Re-auth",
        crate::mcp::McpRecoveryKind::Diagnose => "Diagnose",
    };
    lines.push(format!(
        "  {}",
        crate::mcp::mcp_startup_warning(&server.name, recovery, server.error.is_some())
    ));
    lines.push(format!("  next: {verb} {command}"));
    lines.push(format!(
        "  discovered: {} tools, {} resources, {} prompts",
        server.tools.len(),
        server.resources.len(),
        server.prompts.len()
    ));
    lines.push(format_capability_metadata(
        server.capability_metadata,
        locale,
    ));
    for tool in &server.tools {
        lines.push(format!(
            "    tool {}{}",
            tool.model_name,
            format_mcp_tool_description(tool.description.as_deref())
        ));
    }
    for resource in &server.resources {
        lines.push(format!("    resource {}", resource.name));
    }
    for prompt in &server.prompts {
        lines.push(format!("    prompt {}", prompt.model_name));
    }
}

fn format_capability_metadata(metadata: McpServerCapabilityMetadata, locale: Locale) -> String {
    match metadata {
        McpServerCapabilityMetadata::Advertised(capabilities) => {
            let mut names = Vec::new();
            if capabilities.tools {
                names.push("tools");
            }
            if capabilities.resources {
                names.push("resources");
            }
            if capabilities.prompts {
                names.push("prompts");
            }
            let names = if names.is_empty() {
                tr(locale, MessageId::CoordinationNoneValue).into_owned()
            } else {
                names.join(", ")
            };
            format!(
                "  {}",
                tr(locale, MessageId::McpCapabilitiesAdvertised).replace("{capabilities}", &names)
            )
        }
        McpServerCapabilityMetadata::LegacyFallback => {
            format!("  {}", tr(locale, MessageId::McpCapabilitiesLegacyFallback))
        }
        McpServerCapabilityMetadata::NotObserved => {
            format!("  {}", tr(locale, MessageId::McpCapabilitiesNotObserved))
        }
    }
}

pub(super) fn open_mcp_manager_pager(app: &mut App, snapshot: &McpManagerSnapshot) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(100)
        .saturating_sub(4);
    app.view_stack.push(PagerView::from_text(
        "MCP Manager".to_string(),
        &format_mcp_manager(snapshot, app.ui_locale),
        width.max(60),
    ));
}

pub(super) fn add_mcp_message(app: &mut App, content: String) {
    app.add_message(HistoryCell::System { content });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpDiscoveredItem;
    use std::path::PathBuf;

    #[test]
    fn manager_text_shows_failed_disabled_and_runtime_names() {
        let snapshot = McpManagerSnapshot {
            config_path: PathBuf::from("/tmp/mcp.json"),
            config_exists: true,
            reload_required: true,
            servers: vec![
                McpServerSnapshot {
                    name: "fs".to_string(),
                    enabled: true,
                    required: false,
                    transport: "stdio".to_string(),
                    command_or_url: "node server.js".to_string(),
                    connect_timeout: 10,
                    execute_timeout: 60,
                    read_timeout: 120,
                    connected: true,
                    error: None,
                    capability_metadata: McpServerCapabilityMetadata::Advertised(
                        crate::mcp::McpServerCapabilities {
                            tools: true,
                            resources: false,
                            prompts: false,
                        },
                    ),
                    tools: vec![McpDiscoveredItem {
                        name: "read".to_string(),
                        model_name: "mcp_fs_read".to_string(),
                        description: Some("Read a file".to_string()),
                    }],
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
                McpServerSnapshot {
                    name: "bad".to_string(),
                    enabled: true,
                    required: false,
                    transport: "http/sse".to_string(),
                    command_or_url: "https://example.invalid/mcp".to_string(),
                    connect_timeout: 10,
                    execute_timeout: 60,
                    read_timeout: 120,
                    connected: false,
                    error: Some("boom".to_string()),
                    capability_metadata: McpServerCapabilityMetadata::NotObserved,
                    tools: Vec::new(),
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
            ],
        };
        let text = format_mcp_manager(&snapshot, Locale::En);
        assert!(text.contains("Reload required"));
        assert!(text.contains("/mcp reload"));
        assert!(text.contains("mcp_fs_read"));
        assert!(text.contains("[failed]"));
        assert!(text.contains("boom"));
        assert!(text.contains("Advertised capabilities: tools"));
        assert!(text.contains("not observed because the server is not connected"));
        assert!(text.contains("next: Diagnose /mcp validate"));
        assert!(text.contains("Next: Connect /mcp reload · Diagnose /mcp validate"));
        assert!(!text.contains("/mcp auth"));
    }

    #[test]
    fn manager_text_names_login_for_stale_oauth() {
        let snapshot = McpManagerSnapshot {
            config_path: PathBuf::from("/tmp/mcp.json"),
            config_exists: true,
            reload_required: false,
            servers: vec![McpServerSnapshot {
                name: "github".to_string(),
                enabled: true,
                required: false,
                transport: "http".to_string(),
                command_or_url: "https://api.githubcopilot.com/mcp/".to_string(),
                connect_timeout: 10,
                execute_timeout: 60,
                read_timeout: 120,
                connected: false,
                error: Some("401 Unauthorized".to_string()),
                capability_metadata: McpServerCapabilityMetadata::NotObserved,
                tools: Vec::new(),
                resources: Vec::new(),
                prompts: Vec::new(),
            }],
        };
        let text = format_mcp_manager(&snapshot, Locale::En);
        assert!(text.contains("next: Re-auth /mcp login github"), "{text}");
        assert!(!text.contains("/mcp auth"));
    }

    #[test]
    fn capability_metadata_distinguishes_legacy_fallback_without_scraping_descriptions() {
        let text =
            format_capability_metadata(McpServerCapabilityMetadata::LegacyFallback, Locale::En);

        assert!(text.contains("not provided"), "{text}");
        assert!(text.contains("legacy discovery fallback"), "{text}");
    }

    #[test]
    fn capability_metadata_uses_the_active_ui_locale() {
        let text = format_capability_metadata(
            McpServerCapabilityMetadata::Advertised(crate::mcp::McpServerCapabilities {
                tools: true,
                resources: false,
                prompts: false,
            }),
            Locale::Es419,
        );

        assert!(text.contains("Capacidades anunciadas: tools"), "{text}");
        assert!(!text.contains("Advertised capabilities"), "{text}");
    }
}
