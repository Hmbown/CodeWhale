//! Codewhale bundle lifecycle and legacy executable plugin-tool inventory.
//!
//! `/plugin` owns declarative bundles (`plugin.toml`). Script tools under
//! `[tools].plugin_dir` remain supported, but are labeled as legacy executable
//! tools and never share bundle trust state.
//!
//! # Module map
//!
//! This file is the command surface: registration, the `/plugin` verb
//! dispatch, and the bundle lifecycle verbs (list/show/trust/validate/
//! install/update/uninstall/enable/disable/revoke). Two seams live next
//! door:
//!
//! * [`render`] — every string the user reads: bundle detail, the
//!   capability review body, diagnostics, and the escaping that keeps
//!   manifest-controlled text from forging review output.
//! * [`legacy`] — the separate `[tools].plugin_dir` executable inventory,
//!   which shares no trust state with declarative bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::PathBuf;

use crate::commands::CommandResult;
use crate::commands::traits::{
    Command, CommandGroup, CommandInfo, FunctionCommand, RegisterCommand,
};
use crate::localization::{MessageId, tr};
use crate::plugins::controller::{
    PluginAction, PluginActionOutcome, PluginController, active_network_policy,
};
use crate::plugins::types::{LoadedPlugin, PluginDiagnosticLevel};
use crate::tui::app::{App, AppAction};

mod legacy;
mod render;

#[cfg(test)]
mod tests;

use legacy::{legacy_tools, scan_legacy_tools};
use render::{append_diagnostics, escape_review_path, escape_review_text, render_bundle_detail};

/// Read-only metadata for a legacy executable tool. These scripts use their
/// own per-call approval path and intentionally never share bundle trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LegacyToolApproval {
    Auto,
    Suggest,
    Required,
}

#[derive(Debug, Clone)]
pub(crate) struct LegacyToolInventoryEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub approval: LegacyToolApproval,
}

/// Snapshot the legacy executable-tool directory for the Extensions Manager.
/// It is intentionally read-only: only the existing tool runner performs the
/// scripts' independent approval flow.
pub(crate) fn legacy_tool_inventory(app: &App) -> Vec<LegacyToolInventoryEntry> {
    scan_legacy_tools(app)
        .map(|(_, tools)| {
            tools
                .into_iter()
                .map(|(path, metadata)| LegacyToolInventoryEntry {
                    name: metadata.name,
                    description: metadata.description,
                    path,
                    approval: match metadata.approval {
                        crate::tools::spec::ApprovalRequirement::Auto => LegacyToolApproval::Auto,
                        crate::tools::spec::ApprovalRequirement::Suggest => {
                            LegacyToolApproval::Suggest
                        }
                        crate::tools::spec::ApprovalRequirement::Required => {
                            LegacyToolApproval::Required
                        }
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

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
    usage: "/plugin [list|show|suggest|validate|export|install|update|uninstall|trust|enable|disable|revoke|reload|tools]",
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
        [] => CommandResult::action(AppAction::OpenExtensionsManager),
        ["list"] => list_bundles_and_legacy_tools(app),
        ["help"] => CommandResult::message(tr(app.ui_locale, MessageId::CmdPluginBundleUsage)),
        ["show", selector] => show_bundle(app, selector),
        ["suggest"] | ["recommend"] => CommandResult::error("Usage: /plugin suggest <task>"),
        ["suggest", task @ ..] | ["recommend", task @ ..] => suggest_bundles(app, &task.join(" ")),
        ["validate"] => validate_bundles(app, None),
        ["validate", selector] => validate_bundles(app, Some(selector)),
        ["export"] => CommandResult::error("Usage: /plugin export <name> <target-dir>"),
        ["export", selector, target @ ..] => export_bundle(app, selector, &target.join(" ")),
        ["install"] => CommandResult::error(tr(app.ui_locale, MessageId::CmdPluginBundleUsage)),
        ["install", rest @ ..] => install_bundle(app, &rest.join(" ")),
        ["update"] | ["uninstall"] => {
            CommandResult::error(tr(app.ui_locale, MessageId::CmdPluginBundleUsage))
        }
        ["update", selector] => update_bundle(app, selector),
        ["uninstall", selector] => uninstall_bundle(app, selector),
        ["trust", selector] => review_bundle(app, selector),
        ["trust", selector, token] => mutate_bundle(app, selector, Mutation::Trust(token)),
        ["enable", selector] => mutate_bundle(app, selector, Mutation::Enable),
        ["disable", selector] => mutate_bundle(app, selector, Mutation::Disable),
        ["revoke", selector] => mutate_bundle(app, selector, Mutation::Revoke),
        ["reload"] => execute_action(app, PluginAction::Reload),
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

/// Rank already installed bundle metadata for a task without changing trust,
/// enablement, disk state, or network state. A full remote plugin marketplace
/// needs separately curated publisher/provenance policy; the existing plugin
/// registry is intentionally local-only for this release.
fn suggest_bundles(app: &App, task: &str) -> CommandResult {
    let task = task.trim();
    if task.chars().count() < 3 {
        return CommandResult::error("Usage: /plugin suggest <task of at least 3 characters>");
    }

    let mut skills = BTreeMap::new();
    for plugin in app.plugin_registry.list() {
        let mut description_parts = plugin
            .manifest
            .plugin
            .description
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let mut keywords = Vec::new();
        for skill in &plugin.skill_snapshots {
            description_parts.push(skill.name.clone());
            description_parts.push(skill.description.clone());
            keywords.push(skill.name.clone());
            keywords.extend(skill.aliases.iter().cloned());
        }
        skills.insert(
            plugin.name().to_string(),
            crate::skills::RegistryEntry {
                source: plugin.id.as_str().to_string(),
                description: (!description_parts.is_empty()).then(|| description_parts.join(" ")),
                keywords,
                domains: plugin.inventory.network_hosts.clone(),
            },
        );
    }

    let index = crate::skills::RegistryDocument { skills };
    let recommendations = crate::skills::recommend::recommend_remote_skills(task, &index, 3);
    if recommendations.is_empty() {
        return CommandResult::message(format!(
            "No installed plugin bundles matched `{}`.\n\nInstall a reviewed bundle with /plugin install <source>. Nothing was installed, trusted, or enabled.",
            escape_review_text(task)
        ));
    }

    let mut output = format!(
        "Suggested installed plugins for `{}`:\n",
        escape_review_text(task)
    );
    output.push_str("─────────────────────────────\n");
    for recommendation in recommendations {
        let Some(plugin) = app.plugin_registry.get(&recommendation.entry.source) else {
            continue;
        };
        let description = plugin
            .manifest
            .plugin
            .description
            .as_deref()
            .filter(|description| !description.trim().is_empty())
            .unwrap_or("No description provided.");
        let why = recommendation
            .matched_terms
            .iter()
            .map(|term| escape_review_text(term))
            .collect::<Vec<_>>()
            .join(", ");
        let next_step = if plugin.active() {
            format!("Already active: /plugin show {}", plugin.name())
        } else if !plugin.trusted() {
            format!("Review before enabling: /plugin trust {}", plugin.name())
        } else if !plugin.enabled {
            format!(
                "Enable if that review still applies: /plugin enable {}",
                plugin.name()
            )
        } else {
            format!("Inspect its inactive state: /plugin show {}", plugin.name())
        };
        let _ = writeln!(
            output,
            "  {} — {} · {}",
            escape_review_text(plugin.name()),
            plugin.state_label(),
            escape_review_text(description)
        );
        let _ = writeln!(output, "    Why: {why}");
        let _ = writeln!(output, "    {next_step}");
    }
    output.push_str("\nNothing was installed, trusted, or enabled.");
    CommandResult::message(output)
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

/// `/plugin export <name> <target-dir>` — publish a loaded bundle as a
/// spec-valid Agent Plugins v1.0.0 directory (`plugin.json`, `mcp.json` when
/// servers exist, and the `skills/` tree). The installed bundle is never
/// modified; a relative target resolves against the workspace.
fn export_bundle(app: &App, selector: &str, target: &str) -> CommandResult {
    let Some(plugin) = app.plugin_registry.get(selector).cloned() else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginBundleNotFound).replace("{name}", selector),
        );
    };
    let target = target.trim();
    if target.is_empty() {
        return CommandResult::error("Usage: /plugin export <name> <target-dir>");
    }
    let target = PathBuf::from(target);
    let target = if target.is_absolute() {
        target
    } else {
        app.workspace.join(target)
    };
    let existing_names: BTreeSet<String> = app
        .plugin_registry
        .list()
        .iter()
        .map(|other| other.name().to_string())
        .filter(|name| name != plugin.name())
        .collect();
    match crate::plugins::export::export_plugin_bundle(&plugin, &target, &existing_names) {
        Ok(receipt) => {
            let mut output = format!(
                "Exported `{}` as an Agent Plugins v1.0.0 bundle:\n  {}\n",
                escape_review_text(&receipt.exported_name),
                escape_review_path(&receipt.target),
            );
            if let Some(display_name) = &receipt.display_name {
                let _ = writeln!(
                    output,
                    "  Published under a slugified name; `{}` is preserved as the display name.",
                    escape_review_text(display_name)
                );
            }
            let _ = writeln!(
                output,
                "  plugin.json{} · {} file(s) copied{}",
                if receipt.wrote_mcp_json {
                    " + mcp.json"
                } else {
                    ""
                },
                receipt.files_copied,
                if receipt.skills_normalized {
                    " · skills moved to the standard skills/ layout"
                } else {
                    ""
                }
            );
            output.push_str("The installed bundle was not modified.");
            CommandResult::message(output)
        }
        Err(error) => CommandResult::error(format!(
            "Export of `{}` failed: {}",
            escape_review_text(plugin.name()),
            escape_review_text(&error)
        )),
    }
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
        crate::plugins::controller::review_token(&plugin)
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

// ─── /plugin install | update | uninstall (#5182) ──────────────────────────
//
// The fetch/place on-ramp. All writes go through `plugins::mutation`; after a
// successful install or update the command rediscovers and drops the user
// into the existing trust review (`review_bundle`) — installed or replaced
// bits are always disabled and untrusted until the hash-bound trust flow runs.

fn install_bundle(app: &mut App, spec: &str) -> CommandResult {
    if let Err(error) = crate::plugins::install::PluginInstallSource::parse(spec) {
        return CommandResult::error(format!(
            "Invalid plugin install source `{spec}`: {error:#}\n\
             Expected a local path, github:owner/repo, or an HTTPS tarball URL."
        ));
    }
    execute_action(app, PluginAction::Install { spec: spec.into() })
}

fn update_bundle(app: &mut App, selector: &str) -> CommandResult {
    execute_action(
        app,
        PluginAction::Update {
            selector: selector.into(),
        },
    )
}

fn uninstall_bundle(app: &mut App, selector: &str) -> CommandResult {
    execute_action(
        app,
        PluginAction::Uninstall {
            selector: selector.into(),
        },
    )
}

fn run_async<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // Same bridge as the skill commands: the TUI thread is part of the
    // multi-threaded runtime, so `block_in_place` + `block_on` brings the
    // sync slash-command handler back into the async ecosystem.
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("plugin command runtime")
            .block_on(future)
    }
}

fn needs_approval_message(app: &App, host: &str) -> String {
    tr(app.ui_locale, MessageId::PluginReceiptNeedsNetworkApproval).replace("{host}", host)
}

fn network_denied_message(app: &App, host: &str) -> String {
    tr(app.ui_locale, MessageId::PluginReceiptNetworkDenied).replace("{host}", host)
}

fn mutate_bundle(app: &mut App, selector: &str, mutation: Mutation<'_>) -> CommandResult {
    let action = match mutation {
        Mutation::Trust(token) => PluginAction::Trust {
            selector: selector.into(),
            review_token: token.into(),
        },
        Mutation::Enable => PluginAction::Enable {
            selector: selector.into(),
        },
        Mutation::Disable => PluginAction::Disable {
            selector: selector.into(),
        },
        Mutation::Revoke => PluginAction::Revoke {
            selector: selector.into(),
        },
    };
    execute_action(app, action)
}

#[derive(Clone, Copy)]
enum Mutation<'a> {
    Trust(&'a str),
    Enable,
    Disable,
    Revoke,
}

fn execute_action(app: &mut App, action: PluginAction) -> CommandResult {
    let network = active_network_policy();
    let result = run_async(
        PluginController::new(&mut app.plugin_registry, &app.workspace).execute(action, &network),
    );
    let receipt = match result {
        Ok(receipt) => receipt,
        Err(error) => return action_error(app, &error),
    };
    let changed = receipt.registry_changed;
    let output = match receipt.outcome {
        PluginActionOutcome::Installed { name } => {
            let path = receipt
                .path
                .as_deref()
                .map_or_else(String::new, |path| path.display().to_string());
            app.refresh_skill_cache();
            let mut output = tr(app.ui_locale, MessageId::PluginReceiptInstalledDetailed)
                .replace("{name}", &name)
                .replace("{path}", &path);
            if let Some(review) = review_bundle(app, &name).message {
                output.push('\n');
                output.push_str(&review);
            }
            output
        }
        PluginActionOutcome::Updated { name } => {
            app.refresh_skill_cache();
            let mut output =
                tr(app.ui_locale, MessageId::PluginReceiptUpdatedDetailed).replace("{name}", &name);
            if let Some(review) = review_bundle(app, &name).message {
                output.push('\n');
                output.push_str(&review);
            }
            output
        }
        PluginActionOutcome::AlreadyUpToDate { name } => {
            tr(app.ui_locale, MessageId::PluginReceiptAlreadyUpToDate).replace("{name}", &name)
        }
        PluginActionOutcome::Uninstalled { name } => {
            app.refresh_skill_cache();
            app.active_skill = None;
            app.active_skill_provenance = None;
            tr(app.ui_locale, MessageId::PluginReceiptUninstalled).replace("{name}", &name)
        }
        PluginActionOutcome::NeedsNetworkApproval { host } => {
            return CommandResult::error(needs_approval_message(app, &host));
        }
        PluginActionOutcome::NetworkDenied { host } => {
            return CommandResult::error(network_denied_message(app, &host));
        }
        PluginActionOutcome::Trusted { name } => {
            app.refresh_skill_cache();
            tr(app.ui_locale, MessageId::PluginReceiptTrusted).replace("{name}", &name)
        }
        PluginActionOutcome::Enabled { name } => {
            app.refresh_skill_cache();
            tr(app.ui_locale, MessageId::PluginReceiptEnabled).replace("{name}", &name)
        }
        PluginActionOutcome::Disabled { name } => {
            app.refresh_skill_cache();
            app.active_skill = None;
            app.active_skill_provenance = None;
            tr(app.ui_locale, MessageId::PluginReceiptDisabled).replace("{name}", &name)
        }
        PluginActionOutcome::TrustRevoked { name } => {
            app.refresh_skill_cache();
            app.active_skill = None;
            app.active_skill_provenance = None;
            tr(app.ui_locale, MessageId::PluginReceiptTrustRevoked).replace("{name}", &name)
        }
        PluginActionOutcome::Validated { selector, clean } => {
            let selector = selector.unwrap_or_else(|| {
                tr(app.ui_locale, MessageId::PluginReceiptValidationAll).into_owned()
            });
            let status = tr(
                app.ui_locale,
                if clean {
                    MessageId::PluginReceiptValidationValid
                } else {
                    MessageId::PluginReceiptValidationInvalid
                },
            );
            tr(app.ui_locale, MessageId::PluginReceiptValidation)
                .replace("{target}", &selector)
                .replace("{status}", &status)
        }
        PluginActionOutcome::Reloaded { count } => {
            app.refresh_skill_cache();
            tr(app.ui_locale, MessageId::PluginReceiptReloaded)
                .replace("{count}", &count.to_string())
        }
        PluginActionOutcome::ReviewRequired { name } => {
            return review_bundle(app, &name);
        }
    };
    if changed {
        CommandResult::with_message_and_action(output, AppAction::PluginRegistryChanged)
    } else {
        CommandResult::message(output)
    }
}

fn action_error(app: &App, error: &str) -> CommandResult {
    CommandResult::error(
        tr(app.ui_locale, MessageId::CmdPluginActionFailed).replace("{error}", error),
    )
}
