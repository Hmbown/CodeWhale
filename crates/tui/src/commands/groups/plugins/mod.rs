//! Codewhale bundle lifecycle and legacy executable plugin-tool inventory.
//!
//! `/plugin` owns declarative bundles (`plugin.toml`). Script tools under
//! `[tools].plugin_dir` remain supported, but are labeled as legacy executable
//! tools and never share bundle trust state.

use std::fmt::Write as _;
#[cfg(test)]
use std::fs;
use std::path::{Path, PathBuf};

use crate::commands::CommandResult;
use crate::commands::traits::{
    Command, CommandGroup, CommandInfo, FunctionCommand, RegisterCommand,
};
use crate::localization::{MessageId, tr};
use crate::plugins::types::{LoadedPlugin, PluginDiagnosticLevel};
use crate::tools::plugin::{PluginMetadata, scan_plugin_dir};
use crate::tools::spec::ApprovalRequirement;
use crate::tui::app::{App, AppAction};

pub struct PluginsCommands;

impl CommandGroup for PluginsCommands {
    fn commands(&self) -> &'static [Box<dyn Command>] {
        cached_command_list!(vec![Box::new(FunctionCommand::new(
            PluginsCmd::info(),
            PluginsCmd::execute,
        ))])
    }
}

pub(in crate::commands) const PLUGINS_INFO: CommandInfo = CommandInfo {
    name: "plugin",
    aliases: &["plugins"],
    usage: "/plugin [list|show|validate|trust|enable|disable|revoke|reload|tools]",
    description_id: MessageId::CmdPluginDescription,
};

pub(in crate::commands) struct PluginsCmd;

impl RegisterCommand for PluginsCmd {
    fn info() -> &'static CommandInfo {
        &PLUGINS_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        plugins(app, arg)
    }
}

fn plugins(app: &mut App, arg: Option<&str>) -> CommandResult {
    let words = arg
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    match words.as_slice() {
        [] | ["list"] => list_bundles_and_legacy_tools(app),
        ["help"] => CommandResult::message(tr(app.ui_locale, MessageId::CmdPluginBundleUsage)),
        ["show", selector] => show_bundle(app, selector),
        ["validate"] => validate_bundles(app, None),
        ["validate", selector] => validate_bundles(app, Some(selector)),
        ["trust", selector] => review_bundle(app, selector),
        ["trust", selector, token] => mutate_bundle(app, selector, Mutation::Trust(token)),
        ["enable", selector] => mutate_bundle(app, selector, Mutation::Enable),
        ["disable", selector] => mutate_bundle(app, selector, Mutation::Disable),
        ["revoke", selector] => mutate_bundle(app, selector, Mutation::Revoke),
        ["reload"] => {
            app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&app.workspace);
            app.refresh_skill_cache();
            let count = app.plugin_registry.len();
            CommandResult::with_message_and_action(
                tr(app.ui_locale, MessageId::CmdPluginBundleReloaded)
                    .replace("{count}", &count.to_string())
                    .replace("{workspace}", &app.workspace.display().to_string()),
                AppAction::PluginRegistryChanged,
            )
        }
        ["tools"] => legacy_tools(app, None),
        ["tools", name] => legacy_tools(app, Some(name)),
        [selector] => {
            if app.plugin_registry.get(selector).is_some() {
                show_bundle(app, selector)
            } else {
                // Preserve `/plugin <script-tool>` compatibility while making
                // its distinct execution model explicit in the output.
                legacy_tools(app, Some(selector))
            }
        }
        _ => CommandResult::error(tr(app.ui_locale, MessageId::CmdPluginBundleUsage)),
    }
}

fn list_bundles_and_legacy_tools(app: &App) -> CommandResult {
    let mut output = {
        let registry = app.plugin_registry.as_ref();
        let plugins = registry.list();
        let mut output = if plugins.is_empty() {
            tr(app.ui_locale, MessageId::CmdPluginBundleNoneFound).into_owned()
        } else {
            let mut output = tr(app.ui_locale, MessageId::CmdPluginBundleListHeader)
                .replace("{count}", &plugins.len().to_string());
            output.push('\n');
            for plugin in plugins {
                let _ = writeln!(
                    output,
                    "• {} — {}\n  {} · {} · {}\n  {}",
                    escape_review_text(plugin.name()),
                    plugin.state_label(),
                    plugin.scope,
                    plugin.trust_status.as_str(),
                    plugin.inventory.summary(),
                    escape_review_text(plugin.id.as_str())
                );
            }
            output
        };
        append_diagnostics(app, &mut output, registry.diagnostics());
        output
    };

    if let Some((dir, tools)) = scan_legacy_tools(app) {
        output.push('\n');
        output.push_str(
            &tr(app.ui_locale, MessageId::CmdPluginLegacyListHeader)
                .replace("{count}", &tools.len().to_string())
                .replace("{dir}", &dir.display().to_string()),
        );
        output.push('\n');
        for (path, metadata) in tools {
            let _ = writeln!(
                output,
                "• {} — {}\n  {}",
                escape_review_text(&metadata.name),
                escape_review_text(&metadata.description),
                escape_review_path(&path)
            );
        }
    }

    CommandResult::message(output)
}

fn show_bundle(app: &App, selector: &str) -> CommandResult {
    let Some(plugin) = app.plugin_registry.get(selector).cloned() else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginBundleNotFound).replace("{name}", selector),
        );
    };
    CommandResult::message(render_bundle_detail(app, &plugin, true))
}

fn review_bundle(app: &App, selector: &str) -> CommandResult {
    let Some(plugin) = app.plugin_registry.get(selector).cloned() else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginBundleNotFound).replace("{name}", selector),
        );
    };
    let mut output = render_bundle_detail(app, &plugin, true);
    let _ = writeln!(
        output,
        "\n/plugin trust {} {}",
        plugin.name(),
        review_token(&plugin)
    );
    CommandResult::message(output)
}

fn validate_bundles(app: &App, selector: Option<&str>) -> CommandResult {
    let (plugins, diagnostics, clean) = {
        let registry = app.plugin_registry.as_ref();
        let plugins: Vec<LoadedPlugin> = match selector {
            Some(selector) => registry.get(selector).cloned().into_iter().collect(),
            None => registry.list().into_iter().cloned().collect(),
        };
        (
            plugins,
            registry.diagnostics().to_vec(),
            registry.validation_is_clean(),
        )
    };
    if app.plugin_registry.is_empty() && selector.is_none() {
        return CommandResult::error(tr(app.ui_locale, MessageId::CmdPluginBundleNoneFound));
    };
    if selector.is_some() && plugins.is_empty() {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginBundleNotFound)
                .replace("{name}", selector.unwrap_or_default()),
        );
    }

    let mut output = String::new();
    for plugin in &plugins {
        let _ = writeln!(
            output,
            "{} — {} — {}",
            plugin.name(),
            if plugin
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.level == PluginDiagnosticLevel::Error)
            {
                "invalid"
            } else {
                "valid"
            },
            plugin.inventory.summary()
        );
        append_diagnostics(app, &mut output, &plugin.diagnostics);
    }
    append_diagnostics(app, &mut output, &diagnostics);
    if output.is_empty() {
        output.push_str(if clean { "valid" } else { "invalid" });
    }
    CommandResult::message(output)
}

#[derive(Clone, Copy)]
enum Mutation<'a> {
    Trust(&'a str),
    Enable,
    Disable,
    Revoke,
}

fn mutate_bundle(app: &mut App, selector: &str, mutation: Mutation<'_>) -> CommandResult {
    if matches!(mutation, Mutation::Enable) {
        let needs_review = app
            .plugin_registry
            .get(selector)
            .is_some_and(|plugin| !plugin.trusted());
        if needs_review {
            // Enabling is the natural entry point. Open the exact capability
            // review instead of leaving the user at an opaque denial.
            return review_bundle(app, selector);
        }
    }
    if let Mutation::Trust(token) = mutation {
        let Some(expected) = app.plugin_registry.get(selector).map(review_token) else {
            return CommandResult::error(
                tr(app.ui_locale, MessageId::CmdPluginBundleNotFound).replace("{name}", selector),
            );
        };
        if token != expected {
            return action_error(
                app,
                "Review token does not match this bundle content and capability set; run `/plugin trust <name>` again",
            );
        }
    }

    let result = match mutation {
        Mutation::Trust(_) => std::sync::Arc::make_mut(&mut app.plugin_registry)
            .trust(selector)
            .map(|()| "trusted"),
        Mutation::Enable => std::sync::Arc::make_mut(&mut app.plugin_registry)
            .enable(selector)
            .map(|()| "enabled"),
        Mutation::Disable => std::sync::Arc::make_mut(&mut app.plugin_registry)
            .disable(selector)
            .map(|()| "disabled"),
        Mutation::Revoke => std::sync::Arc::make_mut(&mut app.plugin_registry)
            .revoke_trust(selector)
            .map(|()| "trust-revoked"),
    };
    match result {
        Ok(action) => {
            app.refresh_skill_cache();
            if matches!(mutation, Mutation::Disable | Mutation::Revoke) {
                app.active_skill = None;
                app.active_skill_provenance = None;
            }
            CommandResult::with_message_and_action(
                tr(app.ui_locale, MessageId::CmdPluginBundleMutationSuccess)
                    .replace("{name}", selector)
                    .replace("{action}", action),
                AppAction::PluginRegistryChanged,
            )
        }
        Err(error) => action_error(app, &error),
    }
}

fn render_bundle_detail(app: &App, plugin: &LoadedPlugin, include_hashes: bool) -> String {
    let unsupported = plugin.inventory.unsupported_labels();
    let unsupported = if unsupported.is_empty() {
        "none".to_string()
    } else {
        unsupported.join(", ")
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
        "\nQualified skills: [{}]\nActivation boundary: trust stages the exact reviewed content but does not activate it; enable rebuilds this workspace's Skill/MCP catalog immediately; disable or revoke cancels in-flight plugin MCP operations and denies queued Skills.",
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
                    "{}: transport={} endpoint={} redirects=same-origin-only env_headers=[{}] bearer_env={} oauth=disabled-v0.9.1 timeouts={} required={} enabled_tools=[{}] disabled_tools=[{}] {enabled}",
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

fn escape_review_path(path: &Path) -> String {
    escape_review_text(&path.to_string_lossy())
}

fn escape_review_text(value: &str) -> String {
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

fn review_token(plugin: &LoadedPlugin) -> String {
    // This is an explicit user confirmation, not cosmetic display text. Bind
    // the command to both complete SHA-256 receipts so a same-inventory bundle
    // cannot collide through the former 48-bit content prefix.
    format!("{}.{}", plugin.content_hash, plugin.capability_hash)
}

fn append_diagnostics(
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

fn action_error(app: &App, error: &str) -> CommandResult {
    CommandResult::error(
        tr(app.ui_locale, MessageId::CmdPluginActionFailed).replace("{error}", error),
    )
}

fn legacy_tools(app: &App, name: Option<&str>) -> CommandResult {
    let Some(plugin_dir) = plugin_dir_for(app) else {
        return action_error(
            app,
            "Could not resolve the legacy executable plugin-tool directory",
        );
    };
    if !plugin_dir.exists() {
        return CommandResult::message(
            tr(app.ui_locale, MessageId::CmdPluginNoneFound)
                .replace("{dir}", &plugin_dir.display().to_string()),
        );
    }
    let discovered = scan_plugin_dir(&plugin_dir);
    match name {
        Some(name) => show_legacy_tool_detail(app, name, &discovered),
        None => list_legacy_tools(app, &plugin_dir, &discovered),
    }
}

fn list_legacy_tools(
    app: &App,
    plugin_dir: &Path,
    discovered: &[(PathBuf, PluginMetadata)],
) -> CommandResult {
    if discovered.is_empty() {
        return CommandResult::message(
            tr(app.ui_locale, MessageId::CmdPluginNoneFound)
                .replace("{dir}", &plugin_dir.display().to_string()),
        );
    }
    let mut output = tr(app.ui_locale, MessageId::CmdPluginLegacyListHeader)
        .replace("{count}", &discovered.len().to_string())
        .replace("{dir}", &plugin_dir.display().to_string());
    output.push('\n');
    for (path, metadata) in discovered {
        let _ = writeln!(
            output,
            "• {} — {}\n  {}",
            metadata.name,
            metadata.description,
            path.display()
        );
    }
    CommandResult::message(output)
}

fn show_legacy_tool_detail(
    app: &App,
    name: &str,
    discovered: &[(PathBuf, PluginMetadata)],
) -> CommandResult {
    let Some((path, metadata)) = discovered
        .iter()
        .find(|(_, metadata)| metadata.name == name)
    else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginNotFound).replace("{name}", name),
        );
    };
    let schema = serde_json::to_string_pretty(&metadata.input_schema).unwrap_or_default();
    let mut output = format!("{}\n{:=<40}\n", metadata.name, "");
    let _ = writeln!(
        output,
        "{}",
        tr(app.ui_locale, MessageId::CmdPluginDetailDescription)
            .replace("{description}", &metadata.description)
    );
    let _ = writeln!(
        output,
        "{}",
        tr(app.ui_locale, MessageId::CmdPluginDetailSchema).replace("{schema}", &schema)
    );
    let _ = writeln!(
        output,
        "{}",
        tr(app.ui_locale, MessageId::CmdPluginDetailApproval)
            .replace("{approval}", approval_label(metadata.approval))
    );
    let _ = writeln!(
        output,
        "{}",
        tr(app.ui_locale, MessageId::CmdPluginDetailPath)
            .replace("{path}", &path.display().to_string())
    );
    CommandResult::message(output)
}

fn scan_legacy_tools(app: &App) -> Option<(PathBuf, Vec<(PathBuf, PluginMetadata)>)> {
    let dir = plugin_dir_for(app)?;
    dir.exists().then(|| {
        let tools = scan_plugin_dir(&dir);
        (dir, tools)
    })
}

fn approval_label(approval: ApprovalRequirement) -> &'static str {
    match approval {
        ApprovalRequirement::Auto => "auto",
        ApprovalRequirement::Suggest => "suggest",
        ApprovalRequirement::Required => "required",
    }
}

fn plugin_dir_for(app: &App) -> Option<PathBuf> {
    app.legacy_plugin_tools_dir
        .clone()
        .or_else(default_codewhale_tools_dir)
}

fn default_codewhale_tools_dir() -> Option<PathBuf> {
    codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join("tools"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app(root: &Path) -> (App, TempDir) {
        let temp = TempDir::new().expect("tempdir");
        let config_path = temp.path().join("config.toml");
        let tools_dir = root.join("tools");
        fs::create_dir_all(&tools_dir).unwrap();
        fs::write(
            &config_path,
            format!(
                "[tools]\nplugin_dir = {}\n",
                toml::Value::String(tools_dir.to_string_lossy().to_string())
            ),
        )
        .unwrap();
        let options = TuiOptions {
            config_path: Some(config_path),
            skills_dir: temp.path().join("skills"),
            memory_path: temp.path().join("memory.md"),
            notes_path: temp.path().join("notes.txt"),
            mcp_config_path: temp.path().join("mcp.json"),
            ..crate::test_support::test_tui_options(root)
        };
        let config = Config {
            tools: Some(crate::config::ToolsConfig {
                plugin_dir: Some(tools_dir.to_string_lossy().into_owned()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        let registry = discovery.registry_for_workspace(root);
        let mut app = App::new_with_plugin_registry(options, &config, registry);
        app.ui_locale = Locale::En;
        (app, temp)
    }

    fn write_bundle(root: &Path) {
        let bundle = root.join(".codewhale/plugins/demo");
        fs::create_dir_all(bundle.join("skills/hello")).unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n[skills]\npath = \"skills\"\n",
        )
        .unwrap();
        fs::write(
            bundle.join("skills/hello/SKILL.md"),
            "---\nname: hello\ndescription: hello\n---\nbody\n",
        )
        .unwrap();
    }

    fn write_mcp_review_bundle(root: &Path) {
        let bundle = root.join(".codewhale/plugins/review-mcp");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("server.js"), "// reviewed entrypoint\n").unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            r#"schema_version = 1
[plugin]
name = "review-mcp"
version = "1.0.0"

[mcp_servers.local]
command = "node"
args = ["server.js", "--mode=worker", "-e", "console.log('ready')"]

[mcp_servers.local.env]
PLUGIN_TOKEN = "${PLUGIN_TOKEN_SOURCE}"

[mcp_servers.remote]
url = "https://example.invalid/mcp"
bearer_token_env_var = "REMOTE_TOKEN"

[mcp_servers.remote.env_headers]
X_Api_Key = "REMOTE_API_KEY"

[capabilities]
network_hosts = ["example.invalid"]
"#,
        )
        .unwrap();
    }

    #[test]
    fn list_show_validate_are_read_only_and_label_legacy_tools() {
        let _lock = crate::test_support::lock_test_env();
        let root = TempDir::new().unwrap();
        let codewhale_home = root.path().join("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", &codewhale_home);
        write_bundle(root.path());
        let (mut app, _temp) = create_test_app(root.path());
        fs::write(
            root.path().join("tools/greet.sh"),
            "# name: greet\n# description: hello\n",
        )
        .unwrap();
        // The app already resolved the legacy tools path during startup.
        // Read-only plugin commands must not reopen a credential-bearing
        // config file merely to inventory those tools.
        fs::write(
            app.config_path.as_ref().unwrap(),
            "api_key = [\"must-not-be-re-read\"\n",
        )
        .unwrap();
        let state_path = codewhale_home.join("plugins/state.json");

        for arg in [Some("list"), Some("show demo"), Some("validate")] {
            let result = plugins(&mut app, arg);
            assert!(!result.is_error, "{:?}", result.message);
            assert!(!state_path.exists(), "read-only command wrote plugin state");
        }
        let list = plugins(&mut app, Some("list")).message.unwrap();
        assert!(list.contains("Plugin bundles (1)"));
        assert!(list.contains("disabled"));
        assert!(list.contains("Legacy executable plugin tools (1)"));
    }

    #[test]
    fn trust_requires_content_and_capability_bound_review_token() {
        let _lock = crate::test_support::lock_test_env();
        let root = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
        write_bundle(root.path());
        let (mut app, _temp) = create_test_app(root.path());
        let enable_review = plugins(&mut app, Some("enable demo"));
        assert!(!enable_review.is_error);
        assert!(
            enable_review
                .message
                .as_deref()
                .is_some_and(|message| message.contains("/plugin trust demo "))
        );
        assert!(!app.plugin_registry.get("demo").unwrap().trusted());

        let review = plugins(&mut app, Some("trust demo")).message.unwrap();
        let confirmation = review
            .lines()
            .find(|line| line.starts_with("/plugin trust demo "))
            .unwrap();
        let token = confirmation
            .split_whitespace()
            .last()
            .expect("review confirmation token");
        let (content_digest, capability_digest) = token
            .split_once('.')
            .expect("content and capability digests");
        assert_eq!(content_digest.len(), 64);
        assert_eq!(capability_digest.len(), 64);
        assert!(content_digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(
            capability_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        );
        assert!(!app.plugin_registry.get("demo").unwrap().trusted());

        assert!(plugins(&mut app, Some("trust demo wrong")).is_error);
        let shortened = format!(
            "trust demo {}.{}",
            &content_digest[..12],
            &capability_digest[..12]
        );
        assert!(
            plugins(&mut app, Some(&shortened)).is_error,
            "the legacy 48-bit content prefix must not authorize trust"
        );
        let arg = confirmation.trim_start_matches("/plugin ");
        assert!(!plugins(&mut app, Some(arg)).is_error);
        assert!(!plugins(&mut app, Some("enable demo")).is_error);
        assert!(app.plugin_registry.is_active("demo"));
        assert!(!plugins(&mut app, Some("disable demo")).is_error);
        assert!(!app.plugin_registry.is_active("demo"));
    }

    #[test]
    fn mcp_review_discloses_host_authority_and_names_without_secret_values() {
        let _lock = crate::test_support::lock_test_env();
        let root = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
        write_mcp_review_bundle(root.path());
        let (mut app, _temp) = create_test_app(root.path());
        let review = plugins(&mut app, Some("trust review-mcp"))
            .message
            .expect("review output");
        assert!(review.contains("mcp=2 (stdio=1 remote=1)"));
        assert!(review.contains("host-user filesystem/network authority"));
        assert!(review.contains("PLUGIN\\_TOKEN <- PLUGIN\\_TOKEN\\_SOURCE"));
        assert!(review.contains("X\\_Api\\_Key <- REMOTE\\_API\\_KEY"));
        assert!(review.contains("bearer_env=REMOTE\\_TOKEN"));
        assert!(review.contains("redirects=same-origin-only"));
        assert!(review.contains("Qualified skills: [none]"));
        assert!(review.contains("#2 value=\"--mode=worker\""));
        assert!(review.contains("#3 value=\"-e\""));
        assert!(review.contains("#4 value=\"console.log('ready')\""));
        assert!(review.contains("oauth=disabled-v0.9.1"));
    }

    #[test]
    fn legacy_tool_detail_remains_available_under_tools_namespace() {
        let _lock = crate::test_support::lock_test_env();
        let root = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", root.path().join("home"));
        let (mut app, _temp) = create_test_app(root.path());
        fs::write(
            root.path().join("tools/greet.sh"),
            "# name: greet\n# description: Say hello\n# approval: required\n",
        )
        .unwrap();
        let result = plugins(&mut app, Some("tools greet"));
        assert!(!result.is_error);
        let message = result.message.unwrap();
        assert!(message.contains("Say hello"));
        assert!(message.contains("required"));
    }
}
