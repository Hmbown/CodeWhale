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

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::commands::CommandResult;
use crate::commands::traits::{
    Command, CommandGroup, CommandInfo, FunctionCommand, RegisterCommand,
};
use crate::localization::{MessageId, tr};
use crate::plugins::types::{LoadedPlugin, PluginDiagnosticLevel};
use crate::tui::app::{App, AppAction};

mod kimi_import;
mod legacy;
mod marketplace;
#[cfg(test)]
mod marketplace_tests;
mod render;

#[cfg(test)]
mod tests;

use legacy::{legacy_tools, scan_legacy_tools};
use render::{
    append_diagnostics, escape_review_path, escape_review_text, render_bundle_detail, review_token,
};

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
    aliases: &["plugins", "extensions"],
    usage: "/plugin [list|show|suggest|validate|export|install|import|update|uninstall|trust|enable|disable|revoke|reload|tools|marketplace]",
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
    plugins_with_kimi_home_override(app, arg, None)
}

#[cfg(test)]
fn plugins_with_kimi_home(app: &mut App, arg: Option<&str>, home: &Path) -> CommandResult {
    plugins_with_kimi_home_override(app, arg, Some(home))
}

fn plugins_with_kimi_home_override(
    app: &mut App,
    arg: Option<&str>,
    kimi_home: Option<&Path>,
) -> CommandResult {
    let words = arg
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>();
    match words.as_slice() {
        [] => CommandResult::action(AppAction::OpenExtensions {
            tab: crate::tui::views::extensions::ExtensionsTab::Plugins,
        }),
        ["list"] => list_bundles_and_legacy_tools(app),
        ["help"] => CommandResult::message(format!(
            "{}\n\n/plugin import kimi [list]\n/plugin import kimi approve <name> <content-hash>",
            tr(app.ui_locale, MessageId::CmdPluginBundleUsage)
        )),
        ["marketplace", rest @ ..] => marketplace::dispatch(app, rest),
        ["import", "kimi", rest @ ..] => kimi_import::dispatch(app, rest, kimi_home),
        ["import", ..] => CommandResult::error(kimi_import::usage(app.ui_locale)),
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

/// Rank installed bundles and locally-added marketplace candidates for a task
/// without changing trust, enablement, disk state, or network state.
fn suggest_bundles(app: &App, task: &str) -> CommandResult {
    let task = task.trim();
    if task.chars().count() < 3 {
        return CommandResult::error("Usage: /plugin suggest <task of at least 3 characters>");
    }

    let marketplace =
        crate::plugins::recommend::load_marketplace_candidates(app.plugin_registry.state_path());
    let recommendations = crate::plugins::recommend::recommend_plugins_for_task(
        task,
        app.plugin_registry.as_ref(),
        &marketplace,
        crate::plugins::recommend::RecommendOptions::default(),
    );
    if recommendations.is_empty() {
        return CommandResult::message(format!(
            "No installed or catalog plugin matched `{}`.\n\nInstall a reviewed bundle with /plugin install <source>, or add a catalog with /plugin marketplace add. Nothing was installed, trusted, or enabled.",
            escape_review_text(task)
        ));
    }

    let mut output = format!("Suggested plugins for `{}`:\n", escape_review_text(task));
    output.push_str("─────────────────────────────\n");
    for recommendation in recommendations {
        let description = match &recommendation.source {
            crate::plugins::recommend::PluginMatchSource::Installed { id } => app
                .plugin_registry
                .get(id)
                .and_then(|plugin| plugin.manifest.plugin.description.clone())
                .filter(|description| !description.trim().is_empty())
                .unwrap_or_else(|| "No description provided.".to_string()),
            crate::plugins::recommend::PluginMatchSource::Marketplace { .. } => marketplace
                .iter()
                .find(|candidate| {
                    candidate.name.eq_ignore_ascii_case(&recommendation.name)
                        && matches!(
                            &recommendation.source,
                            crate::plugins::recommend::PluginMatchSource::Marketplace { catalog_id }
                                if candidate.catalog_id.as_str() == catalog_id
                        )
                })
                .and_then(|candidate| candidate.description.clone())
                .filter(|description| !description.trim().is_empty())
                .unwrap_or_else(|| "Catalog plugin.".to_string()),
        };
        let state = match &recommendation.source {
            crate::plugins::recommend::PluginMatchSource::Installed { id } => app
                .plugin_registry
                .get(id)
                .map(|plugin| plugin.state_label())
                .unwrap_or("installed"),
            crate::plugins::recommend::PluginMatchSource::Marketplace { .. } => "not installed",
        };
        let why = recommendation
            .matched_terms
            .iter()
            .map(|term| escape_review_text(term))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            output,
            "  {} — {state} · {}",
            escape_review_text(&recommendation.name),
            escape_review_text(&description)
        );
        let _ = writeln!(output, "    Why: {why}");
        let _ = writeln!(output, "    {}", recommendation.command());
    }
    output.push_str("\nNothing was installed, trusted, or enabled.");
    CommandResult::message(output)
}

fn list_bundles_and_legacy_tools(app: &mut App) -> CommandResult {
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
                    "• {} — {}\n  {} · {} · compatibility={} · {}\n  {}",
                    escape_review_text(plugin.name()),
                    plugin.state_label(),
                    plugin.scope,
                    plugin.trust_status.as_str(),
                    plugin.compatibility().as_str(),
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

    if let Some(nudge) = crate::plugins::plugin_reload_nudge(
        app.plugin_registry.as_ref(),
        &mut app.plugin_reload_nudge_stamp,
    ) {
        output.push('\n');
        output.push_str(nudge);
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

// ─── /plugin install | update | uninstall (#5182) ──────────────────────────
//
// The fetch/place on-ramp. All writes go through `plugins::mutation`; after a
// successful install or update the command rediscovers and drops the user
// into the existing trust review (`review_bundle`) — installed or replaced
// bits are always disabled and untrusted until the hash-bound trust flow runs.

fn install_bundle(app: &mut App, spec: &str) -> CommandResult {
    let source = match crate::plugins::install::PluginInstallSource::parse(spec) {
        Ok(source) => source,
        Err(error) => {
            return CommandResult::error(format!(
                "Invalid plugin install source `{spec}`: {error:#}\n\
                 Expected a local path, github:owner/repo, or an HTTPS tarball URL."
            ));
        }
    };
    install_bundle_source(app, source, None)
}

fn install_bundle_with_expected_hash(
    app: &mut App,
    path: &std::path::Path,
    expected_content_hash: &str,
) -> CommandResult {
    install_bundle_source(
        app,
        crate::plugins::install::PluginInstallSource::LocalPath(path.to_path_buf()),
        Some(expected_content_hash),
    )
}

fn install_bundle_source(
    app: &mut App,
    source: crate::plugins::install::PluginInstallSource,
    expected_content_hash: Option<&str>,
) -> CommandResult {
    use crate::plugins::mutation::{
        PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
    };

    let network = plugin_network_policy();
    let expected_content_hash = expected_content_hash.map(str::to_string);
    let expected_for_request = expected_content_hash.clone();
    let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
    let outcome = run_async(async move {
        let ctx = PluginMutationContext {
            network: &network,
            max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
        };
        let request = match expected_for_request {
            Some(expected_content_hash) => PluginMutationRequest::InstallExact {
                source,
                expected_content_hash,
            },
            None => PluginMutationRequest::Install { source },
        };
        crate::plugins::mutation::execute(request, &ctx, registry).await
    });

    match outcome {
        Ok(receipt) => match receipt.outcome {
            PluginMutationOutcome::Installed => {
                let name = receipt.name.clone();
                let installed_path = receipt.path.clone();
                let installed_content_hash = receipt.installed_content_hash.clone();
                let path = installed_path
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default();
                if let Some(expected) = expected_content_hash.as_deref()
                    && receipt.content_hash.as_deref() != Some(expected)
                {
                    return rollback_hash_mismatch(
                        app,
                        &name,
                        installed_path.as_deref(),
                        expected,
                        receipt.content_hash.as_deref(),
                    );
                }
                app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&app.workspace);
                app.refresh_skill_cache();
                if expected_content_hash.is_some() {
                    let post_copy_hash = app
                        .plugin_registry
                        .get(&name)
                        .map(|plugin| plugin.content_hash.clone());
                    if installed_content_hash.is_none()
                        || post_copy_hash.as_deref() != installed_content_hash.as_deref()
                    {
                        return rollback_hash_mismatch(
                            app,
                            &name,
                            installed_path.as_deref(),
                            installed_content_hash.as_deref().unwrap_or("unavailable"),
                            post_copy_hash.as_deref(),
                        );
                    }
                }
                let mut output = format!(
                    "Installed plugin '{name}' to {path}.\n\
                     It is disabled and untrusted. Review its requested authority below, then trust and enable it.\n"
                );
                if let Some(review) = review_bundle(app, &name).message {
                    output.push('\n');
                    output.push_str(&review);
                }
                CommandResult::with_message_and_action(output, AppAction::PluginRegistryChanged)
            }
            PluginMutationOutcome::NeedsApproval(host) => {
                CommandResult::error(needs_approval_message(&host))
            }
            PluginMutationOutcome::NetworkDenied(host) => {
                CommandResult::error(network_denied_message(&host))
            }
            other => CommandResult::error(format!("Unexpected install outcome: {other:?}")),
        },
        Err(error) => action_error(app, &format!("Plugin install failed: {error:#}")),
    }
}

fn rollback_hash_mismatch(
    app: &mut App,
    name: &str,
    installed_path: Option<&std::path::Path>,
    expected: &str,
    actual: Option<&str>,
) -> CommandResult {
    let locale = app.ui_locale;
    let missing_destination =
        tr(locale, MessageId::PluginKimiRollbackDestinationMissing).into_owned();
    let rollback = installed_path
        .and_then(std::path::Path::parent)
        .ok_or_else(|| anyhow::anyhow!(missing_destination))
        .and_then(|plugins_dir| crate::plugins::install::uninstall(name, plugins_dir));
    app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&app.workspace);
    app.refresh_skill_cache();
    let actual = actual
        .map(escape_review_text)
        .unwrap_or_else(|| tr(locale, MessageId::PluginKimiHashUnavailable).into_owned());
    let name = escape_review_text(name);
    let expected = escape_review_text(expected);
    match rollback {
        Ok(()) => CommandResult::error(
            tr(locale, MessageId::PluginKimiMismatchRemoved)
                .replace("{name}", &name)
                .replace("{expected}", &expected)
                .replace("{actual}", &actual),
        ),
        Err(error) => CommandResult {
            message: Some(
                tr(locale, MessageId::PluginKimiMismatchRollbackFailed)
                    .replace("{name}", &name)
                    .replace("{expected}", &expected)
                    .replace("{actual}", &actual)
                    .replace("{error}", &escape_review_text(&format!("{error:#}")))
                    .replace(
                        "{path}",
                        &installed_path.map(escape_review_path).unwrap_or_else(|| {
                            tr(locale, MessageId::PluginKimiUserPluginDirectory).into_owned()
                        }),
                    ),
            ),
            action: Some(AppAction::PluginRegistryChanged),
            is_error: true,
        },
    }
}

fn update_bundle(app: &mut App, selector: &str) -> CommandResult {
    use crate::plugins::mutation::{
        PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
    };

    let network = plugin_network_policy();
    let selector_owned = selector.to_string();
    let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
    let outcome = run_async(async move {
        let ctx = PluginMutationContext {
            network: &network,
            max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
        };
        crate::plugins::mutation::execute(
            PluginMutationRequest::Update {
                selector: selector_owned,
            },
            &ctx,
            registry,
        )
        .await
    });

    match outcome {
        Ok(receipt) => match receipt.outcome {
            PluginMutationOutcome::Updated => {
                let name = receipt.name.clone();
                app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&app.workspace);
                app.refresh_skill_cache();
                let mut output = format!(
                    "Updated plugin '{name}'. Its content changed, so the previous trust receipt no \
                     longer matches — review and trust it again before enabling.\n"
                );
                if let Some(review) = review_bundle(app, &name).message {
                    output.push('\n');
                    output.push_str(&review);
                }
                CommandResult::with_message_and_action(output, AppAction::PluginRegistryChanged)
            }
            PluginMutationOutcome::NoChange => {
                CommandResult::message(format!("Plugin '{}' is already up to date.", receipt.name))
            }
            PluginMutationOutcome::NeedsApproval(host) => {
                CommandResult::error(needs_approval_message(&host))
            }
            PluginMutationOutcome::NetworkDenied(host) => {
                CommandResult::error(network_denied_message(&host))
            }
            other => CommandResult::error(format!("Unexpected update outcome: {other:?}")),
        },
        Err(error) => action_error(app, &format!("Plugin update failed: {error:#}")),
    }
}

fn uninstall_bundle(app: &mut App, selector: &str) -> CommandResult {
    use crate::plugins::mutation::{
        PluginMutationContext, PluginMutationOutcome, PluginMutationRequest,
    };

    let network = plugin_network_policy();
    let selector_owned = selector.to_string();
    let registry = std::sync::Arc::make_mut(&mut app.plugin_registry);
    let outcome = run_async(async move {
        let ctx = PluginMutationContext {
            network: &network,
            max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
        };
        crate::plugins::mutation::execute(
            PluginMutationRequest::Uninstall {
                selector: selector_owned,
            },
            &ctx,
            registry,
        )
        .await
    });

    match outcome {
        Ok(receipt) => {
            debug_assert!(matches!(
                receipt.outcome,
                PluginMutationOutcome::Uninstalled
            ));
            app.plugin_registry = app.plugin_registry.rediscover_for_workspace(&app.workspace);
            app.refresh_skill_cache();
            app.active_skill = None;
            app.active_skill_provenance = None;
            CommandResult::with_message_and_action(
                format!("Uninstalled plugin '{}'.", receipt.name),
                AppAction::PluginRegistryChanged,
            )
        }
        Err(error) => action_error(app, &format!("Plugin uninstall failed: {error:#}")),
    }
}

/// Read the active network policy for plugin downloads. Mirrors the skill
/// installer's on-demand `Config::load` (`App` carries no `Config` field);
/// a parse failure falls back to the prompt-default policy so the download
/// stays gated rather than crashing.
fn plugin_network_policy() -> crate::network_policy::NetworkPolicy {
    crate::config::Config::load(None, None)
        .unwrap_or_default()
        .network
        .map(|policy| policy.into_runtime())
        .unwrap_or_default()
}

fn run_async<F, T>(future: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // Same bridge as the skill commands: the TUI thread is part of the
    // multi-threaded runtime, so `block_in_place` + `block_on` brings the
    // sync slash-command handler back into the async ecosystem.
    tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(future))
}

fn needs_approval_message(host: &str) -> String {
    format!(
        "Network policy requires approval for {host}.\n\
         Add it to your allow list with `/network allow {host}` (or set [network].default = \"allow\" in ~/.codewhale/config.toml), then retry."
    )
}

fn network_denied_message(host: &str) -> String {
    format!(
        "Network policy denied access to {host}.\n\
         Remove the deny entry from ~/.codewhale/config.toml under [network] or contact your administrator."
    )
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
            let mut message = tr(app.ui_locale, MessageId::CmdPluginBundleMutationSuccess)
                .replace("{name}", selector)
                .replace("{action}", action);
            if matches!(mutation, Mutation::Enable)
                && let Some(plugin) = app.plugin_registry.get(selector)
            {
                let inactive = plugin.inventory.unsupported_labels();
                if !inactive.is_empty() {
                    message.push(' ');
                    message.push_str(&format!(
                        "Compatibility: {}. Supported declarative components are active; inactive: {}.",
                        plugin.compatibility().as_str(),
                        inactive.join(", ")
                    ));
                }
            }
            CommandResult::with_message_and_action(message, AppAction::PluginRegistryChanged)
        }
        Err(error) => action_error(app, &error),
    }
}

fn action_error(app: &App, error: &str) -> CommandResult {
    CommandResult::error(
        tr(app.ui_locale, MessageId::CmdPluginActionFailed).replace("{error}", error),
    )
}
