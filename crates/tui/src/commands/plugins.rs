//! `/plugins` slash command — list and inspect script plugin tools.

use std::path::PathBuf;

use crate::commands::CommandResult;
use crate::config::Config;
use crate::localization::{MessageId, tr};
use crate::tui::app::App;
use crate::tools::plugin::scan_plugin_dir;

/// List discovered plugins, or show details for a named plugin.
pub fn plugins(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(plugin_dir) = plugin_dir_for(app) else {
        return CommandResult::error(
            "Could not resolve plugin directory. Set [tools].plugin_dir in config.toml or ensure ~/.codewhale/tools exists.".to_string(),
        );
    };

    if !plugin_dir.exists() {
        return CommandResult::message(format!(
            "No plugin directory found at {}",
            plugin_dir.display()
        ));
    }

    let discovered = scan_plugin_dir(&plugin_dir);

    if let Some(name) = arg.map(str::trim).filter(|s| !s.is_empty()) {
        show_plugin_detail(app, name, &discovered)
    } else {
        list_plugins(app, &plugin_dir, &discovered)
    }
}

fn list_plugins(
    app: &App,
    plugin_dir: &std::path::Path,
    discovered: &[(PathBuf, crate::tools::plugin::PluginMetadata)],
) -> CommandResult {
    if discovered.is_empty() {
        return CommandResult::message(
            tr(app.ui_locale, MessageId::CmdPluginNoneFound)
                .replace("{dir}", &plugin_dir.display().to_string()),
        );
    }

    let mut out = String::new();
    out.push_str(
        &tr(app.ui_locale, MessageId::CmdPluginListHeader)
            .replace("{count}", &discovered.len().to_string()),
    );
    out.push('\n');

    for (path, meta) in discovered {
        out.push_str(&format!(
            "• {} — {}\n  {}",
            meta.name,
            meta.description,
            path.display()
        ));
        out.push('\n');
    }

    CommandResult::message(out)
}

fn show_plugin_detail(
    app: &App,
    name: &str,
    discovered: &[(PathBuf, crate::tools::plugin::PluginMetadata)],
) -> CommandResult {
    let Some((path, meta)) = discovered.iter().find(|(_, m)| m.name == name) else {
        return CommandResult::error(
            tr(app.ui_locale, MessageId::CmdPluginNotFound).replace("{name}", name),
        );
    };

    let schema = serde_json::to_string_pretty(&meta.input_schema).unwrap_or_default();
    let approval = approval_label(meta.approval);

    let mut out = String::new();
    out.push_str(&format!("{}\n", meta.name));
    out.push_str(&format!("{:=<40}\n", ""));
    out.push_str(&format!(
        "{}\n",
        tr(app.ui_locale, MessageId::CmdPluginDetailDescription)
            .replace("{description}", &meta.description)
    ));
    out.push_str(&format!(
        "{}\n",
        tr(app.ui_locale, MessageId::CmdPluginDetailSchema).replace("{schema}", &schema)
    ));
    out.push_str(&format!(
        "{}\n",
        tr(app.ui_locale, MessageId::CmdPluginDetailApproval)
            .replace("{approval}", approval)
    ));
    out.push_str(&format!(
        "{}\n",
        tr(app.ui_locale, MessageId::CmdPluginDetailPath)
            .replace("{path}", &path.display().to_string())
    ));

    CommandResult::message(out)
}

fn approval_label(approval: crate::tools::spec::ApprovalRequirement) -> &'static str {
    match approval {
        crate::tools::spec::ApprovalRequirement::Auto => "auto",
        crate::tools::spec::ApprovalRequirement::Suggest => "suggest",
        crate::tools::spec::ApprovalRequirement::Required => "required",
    }
}

/// Resolve the configured plugin directory, defaulting to `~/.codewhale/tools`.
fn plugin_dir_for(app: &App) -> Option<PathBuf> {
    let config = match &app.config_path {
        Some(path) => Config::load(Some(path.clone()), app.config_profile.as_deref())
            .unwrap_or_default(),
        None => Config::default(),
    };

    config
        .tools
        .as_ref()
        .and_then(|tools| tools.plugin_dir.as_ref())
        .map(PathBuf::from)
        .or_else(default_codewhale_tools_dir)
}

fn default_codewhale_tools_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codewhale").join("tools"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_plugin_dir(plugin_dir: &std::path::Path) -> (App, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        let tools_dir = plugin_dir.canonicalize().unwrap_or_else(|_| plugin_dir.to_path_buf());
        std::fs::write(
            &config_path,
            format!(
                "[tools]\nplugin_dir = {}\n",
                toml::Value::String(tools_dir.to_string_lossy().to_string())
            ),
        )
        .expect("write config");

        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmp.path().to_path_buf(),
            config_path: Some(config_path),
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmp.path().join("skills"),
            memory_path: tmp.path().join("memory.md"),
            notes_path: tmp.path().join("notes.txt"),
            mcp_config_path: tmp.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let app = App::new(options, &Config::default());
        (app, tmp)
    }

    #[test]
    fn test_plugins_lists_discovered_tools() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("greet.sh"),
            "# name: greet\n# description: Say hello\n# schema: {\"type\":\"object\"}\n# approval: auto\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("audit.sh"),
            "# name: audit\n# description: Audit wrapper\n# approval: required\n",
        )
        .unwrap();

        let (mut app, _tmp) = create_test_app_with_plugin_dir(dir.path());
        let result = plugins(&mut app, None);
        let msg = result.message.expect("should return list");
        assert!(msg.contains("Plugin tools (2):"));
        assert!(msg.contains("greet"));
        assert!(msg.contains("Say hello"));
        assert!(msg.contains("audit"));
        assert!(msg.contains("Audit wrapper"));
        assert!(msg.contains("greet.sh"));
        assert!(!result.is_error);
    }

    #[test]
    fn test_plugins_empty_directory() {
        let dir = TempDir::new().unwrap();
        let (mut app, _tmp) = create_test_app_with_plugin_dir(dir.path());
        let result = plugins(&mut app, None);
        let msg = result.message.expect("should return message");
        assert!(msg.contains("No plugin tools discovered"));
        assert!(msg.contains(&dir.path().canonicalize().unwrap().display().to_string()));
        assert!(!result.is_error);
    }

    #[test]
    fn test_plugins_detail_shows_metadata() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("tool.sh"),
            "# name: my-tool\n# description: Does a thing\n# schema: {\"type\":\"object\",\"properties\":{\"x\":{\"type\":\"string\"}}}\n# approval: required\n",
        )
        .unwrap();

        let (mut app, _tmp) = create_test_app_with_plugin_dir(dir.path());
        let result = plugins(&mut app, Some("my-tool"));
        let msg = result.message.expect("should return detail");
        assert!(msg.contains("my-tool"));
        assert!(msg.contains("Does a thing"));
        assert!(msg.contains("\"type\": \"object\""));
        assert!(msg.contains("\"x\""));
        assert!(msg.contains("required"));
        assert!(msg.contains("tool.sh"));
        assert!(!result.is_error);
    }

    #[test]
    fn test_plugins_detail_not_found() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("existing.sh"),
            "# name: existing\n# description: exists\n",
        )
        .unwrap();

        let (mut app, _tmp) = create_test_app_with_plugin_dir(dir.path());
        let result = plugins(&mut app, Some("missing"));
        assert!(result.is_error);
        let msg = result.message.expect("should return error");
        assert!(msg.contains("missing"));
        assert!(msg.contains("not found"));
    }
}
