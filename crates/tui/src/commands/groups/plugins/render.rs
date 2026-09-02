//! Presentation for `/plugin`: bundle detail, the capability review body,
//! and diagnostics.
//!
//! Everything here is a pure `&LoadedPlugin -> String` transform — no
//! registry mutation, no disk access. [`escape_review_text`] is the
//! security-relevant part: manifest fields are attacker-controlled, so they
//! are escaped before they reach a review the user is about to approve.

use std::fmt::Write as _;
use std::path::Path;

use crate::localization::{MessageId, tr};
use crate::plugins::types::{LoadedPlugin, PluginDiagnosticLevel};
use crate::tui::app::App;

pub(super) fn render_bundle_detail(
    app: &App,
    plugin: &LoadedPlugin,
    include_hashes: bool,
) -> String {
    let unsupported = plugin.inventory.unsupported_labels();
    let unsupported = if unsupported.is_empty() {
        "none".to_string()
    } else {
        unsupported.join(", ")
    };
    let active_components = if plugin.active() {
        let labels = plugin.inventory.supported_labels();
        if labels.is_empty() {
            "none".to_string()
        } else {
            labels.join(", ")
        }
    } else {
        "none".to_string()
    };
    let (content_hash, capability_hash) = if include_hashes {
        (
            plugin.content_hash.as_str(),
            plugin.capability_hash.as_str(),
        )
    } else {
        ("hidden", "hidden")
    };
    let mut output = tr(app.ui_locale, MessageId::CmdPluginBundleDetail)
        .replace("{name}", &escape_review_text(plugin.name()))
        .replace("{id}", &escape_review_text(plugin.id.as_str()))
        .replace(
            "{version}",
            &escape_review_text(&plugin.manifest.plugin.version),
        )
        .replace("{origin}", plugin.origin.as_str())
        .replace("{scope}", plugin.scope.as_str())
        .replace("{state}", plugin.state_label())
        .replace("{trust}", plugin.trust_status.as_str())
        .replace("{inventory}", &plugin.inventory.summary())
        .replace("{permissions}", &render_permissions(plugin))
        .replace("{mcp}", &render_mcp_inventory(plugin))
        .replace("{unsupported}", &unsupported)
        .replace("{content_hash}", content_hash)
        .replace("{capability_hash}", capability_hash)
        .replace("{path}", &escape_review_path(&plugin.canonical_root));
    let skills = plugin
        .skill_snapshots
        .iter()
        .map(|skill| escape_review_text(&format!("{}:{}", plugin.name(), skill.name)))
        .collect::<Vec<_>>();
    let _ = write!(
        output,
        "\nCompatibility: {}\nActive components: [{active_components}]\nInactive components: [{unsupported}]\nQualified skills: [{}]\nActivation boundary: trust stages the exact reviewed content but does not activate it; enable rebuilds this workspace's Skills, MCP, Commands, Agents, and Hooks immediately. Every plugin command dispatch, Agent spawn, Hook process start, Skill use, and MCP call rechecks current authority. LSP, native, filesystem-roots, and lifecycle-mutation stay inventoried and inactive.",
        plugin.compatibility().as_str(),
        if skills.is_empty() {
            "none".to_string()
        } else {
            skills.join(", ")
        }
    );
    append_diagnostics(app, &mut output, &plugin.diagnostics);
    output
}

fn render_permissions(plugin: &LoadedPlugin) -> String {
    let filesystem = if plugin.inventory.filesystem_roots.is_empty() {
        "none".to_string()
    } else {
        plugin
            .inventory
            .filesystem_roots
            .iter()
            .map(|value| escape_review_text(value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let network = if plugin.inventory.network_hosts.is_empty() {
        "none".to_string()
    } else {
        plugin
            .inventory
            .network_hosts
            .iter()
            .map(|value| escape_review_text(value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let stdio_authority = if plugin.inventory.stdio_mcp_servers == 0 {
        "none".to_string()
    } else {
        format!(
            "{} local child process(es) with host-user filesystem/network authority; MCP tool approvals still apply",
            plugin.inventory.stdio_mcp_servers
        )
    };
    format!(
        "filesystem_roots=[{filesystem}] network_hosts=[{network}] (exact allowlist for Codewhale-managed remote requests; redirects stay same-origin) lifecycle_mutation={} stdio_runtime=[{stdio_authority}]",
        plugin.inventory.lifecycle_mutation
    )
}

fn render_mcp_inventory(plugin: &LoadedPlugin) -> String {
    let Some(servers) = plugin.manifest.mcp_servers.as_ref() else {
        return "none".to_string();
    };
    let mut servers = servers.iter().collect::<Vec<_>>();
    servers.sort_by_key(|(name, _)| *name);
    servers
        .into_iter()
        .map(|(name, server)| {
            let enabled = if server.is_enabled() {
                "configured-on"
            } else {
                "configured-off"
            };
            if let Some(command) = server.command.as_deref() {
                let mut env_provenance = server
                    .env
                    .iter()
                    .map(|(destination, source)| {
                        let source = source
                            .strip_prefix("${")
                            .and_then(|source| source.strip_suffix('}'))
                            .unwrap_or("invalid");
                        format!(
                            "{} <- {}",
                            escape_review_text(destination),
                            escape_review_text(source)
                        )
                    })
                    .collect::<Vec<_>>();
                env_provenance.sort_unstable();
                let cwd = server
                    .cwd
                    .as_deref()
                    .map(escape_review_path)
                    .unwrap_or_else(|| "plugin-root".to_string());
                let argv = render_review_argv(plugin, &server.args);
                format!(
                    "{}: transport=stdio command={} argv=[{}] cwd={cwd} env=[{}] timeouts={} required={} enabled_tools=[{}] disabled_tools=[{}] host-user-filesystem/network-authority {enabled}",
                    escape_review_text(name),
                    escape_review_text(command),
                    argv.join(", "),
                    if env_provenance.is_empty() { "none".to_string() } else { env_provenance.join(", ") },
                    render_mcp_timeouts(server),
                    server.required,
                    render_review_values(&server.enabled_tools),
                    render_review_values(&server.disabled_tools),
                )
            } else if let Some(url) = server.url.as_deref() {
                let endpoint = reqwest::Url::parse(url)
                    .ok()
                    .map(|url| escape_review_text(url.as_str()))
                    .unwrap_or_else(|| "invalid-url".to_string());
                let mut env_headers = server
                    .env_headers
                    .iter()
                    .map(|(header, source)| {
                        format!(
                            "{} <- {}",
                            escape_review_text(header),
                            escape_review_text(source)
                        )
                    })
                    .collect::<Vec<_>>();
                env_headers.sort_unstable();
                let bearer = server
                    .bearer_token_env_var
                    .as_deref()
                    .map(escape_review_text)
                    .unwrap_or_else(|| "none".to_string());
                let transport = server.transport.as_deref().unwrap_or(
                    "streamable-http with same-origin SSE fallback",
                );
                format!(
                    "{}: transport={} endpoint={} redirects=same-origin-only env_headers=[{}] bearer_env={} oauth=disabled timeouts={} required={} enabled_tools=[{}] disabled_tools=[{}] {enabled}",
                    escape_review_text(name),
                    escape_review_text(transport),
                    endpoint,
                    if env_headers.is_empty() { "none".to_string() } else { env_headers.join(", ") },
                    bearer,
                    render_mcp_timeouts(server),
                    server.required,
                    render_review_values(&server.enabled_tools),
                    render_review_values(&server.disabled_tools),
                )
            } else {
                format!("{name}: invalid")
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn render_review_argv(plugin: &LoadedPlugin, arguments: &[String]) -> Vec<String> {
    arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            let position = index + 1;
            let candidate = plugin.canonical_root.join(argument);
            if candidate.exists()
                && candidate
                    .canonicalize()
                    .is_ok_and(|path| path.starts_with(&plugin.canonical_root))
            {
                return format!(
                    "#{position} plugin-path={}",
                    render_review_argv_value(argument)
                );
            }
            format!("#{position} value={}", render_review_argv_value(argument))
        })
        .collect()
}

fn render_review_argv_value(value: &str) -> String {
    // JSON string syntax is a lossless, unambiguous terminal representation:
    // whitespace, quotes, backslashes, and punctuation retain their exact
    // argv semantics without hiding arbitrary values behind redaction.
    serde_json::to_string(value).expect("serializing a Rust string cannot fail")
}

fn render_review_values(values: &[String]) -> String {
    if values.is_empty() {
        return "none".to_string();
    }
    values
        .iter()
        .map(|value| escape_review_text(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_mcp_timeouts(server: &crate::mcp::McpServerConfig) -> String {
    format!(
        "connect={}/execute={}/read={}",
        server
            .connect_timeout
            .map_or_else(|| "default".to_string(), |value| format!("{value}s")),
        server
            .execute_timeout
            .map_or_else(|| "default".to_string(), |value| format!("{value}s")),
        server
            .read_timeout
            .map_or_else(|| "default".to_string(), |value| format!("{value}s")),
    )
}

pub(super) fn escape_review_path(path: &Path) -> String {
    escape_review_text(&path.to_string_lossy())
}

pub(super) fn escape_review_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control()
            || matches!(
                ch,
                '\u{061c}'
                    | '\u{200e}'
                    | '\u{200f}'
                    | '\u{202a}'..='\u{202e}'
                    | '\u{2066}'..='\u{2069}'
            )
        {
            let _ = write!(escaped, "\\u{{{:x}}}", ch as u32);
        } else if matches!(
            ch,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
        ) {
            escaped.push('\\');
            escaped.push(ch);
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

pub(super) fn review_token(plugin: &LoadedPlugin) -> String {
    // One implementation lives on `LoadedPlugin`; the TUI command and the
    // Runtime API trust endpoint must agree byte-for-byte.
    plugin.review_token()
}

pub(super) fn append_diagnostics(
    app: &App,
    output: &mut String,
    diagnostics: &[crate::plugins::types::PluginDiagnostic],
) {
    if diagnostics.is_empty() {
        return;
    }
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(
        &tr(app.ui_locale, MessageId::CmdPluginBundleDiagnosticsHeader)
            .replace("{count}", &diagnostics.len().to_string()),
    );
    output.push('\n');
    for diagnostic in diagnostics {
        let level = match diagnostic.level {
            PluginDiagnosticLevel::Warning => "warning",
            PluginDiagnosticLevel::Error => "error",
        };
        let path = diagnostic
            .path
            .as_deref()
            .map(|path| format!(" ({})", escape_review_path(path)))
            .unwrap_or_default();
        let _ = writeln!(
            output,
            "• {level} [{}]: {}{path}",
            diagnostic.code,
            escape_review_text(&diagnostic.message)
        );
    }
}
