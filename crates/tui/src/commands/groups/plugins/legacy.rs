//! Legacy executable plugin-tool inventory (`[tools].plugin_dir`).
//!
//! These are scripts, not declarative bundles: they are discovered by
//! scanning a directory, they carry their own approval requirement, and
//! they never share bundle trust state. `/plugin tools` reports them
//! read-only — nothing here installs, trusts, or executes anything.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::commands::CommandResult;
use crate::localization::{MessageId, tr};
use crate::tools::plugin::{PluginMetadata, scan_plugin_dir};
use crate::tools::spec::ApprovalRequirement;
use crate::tui::app::App;

use super::action_error;

pub(super) fn legacy_tools(app: &App, name: Option<&str>) -> CommandResult {
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

pub(super) fn scan_legacy_tools(app: &App) -> Option<(PathBuf, Vec<(PathBuf, PluginMetadata)>)> {
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
