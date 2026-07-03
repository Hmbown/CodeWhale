//! Tools / MCP / Skills / Plugins onboarding step (#3407).
//!
//! Introduces the extensibility surface during first-run onboarding with a
//! read-only inventory. Configuration and any side-effectful verification stay
//! in `/setup`, `/mcp`, `/skills`, and `/plugins`.

use std::path::PathBuf;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::config::Config;
use crate::localization::MessageId;
use crate::mcp::{McpConfig, McpManagerSnapshot};
use crate::palette;
use crate::tools::plugin::scan_plugin_dir;
use crate::tui::app::App;
use crate::utils::display_path;

pub fn lines(app: &App) -> Vec<Line<'static>> {
    let inventory = Inventory::from_app(app);

    vec![
        Line::from(Span::styled(
            app.tr(MessageId::OnboardToolsMcpTitle).to_string(),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::raw(
            app.tr(MessageId::OnboardToolsMcpLine1).to_string(),
        )),
        Line::from(Span::raw(
            app.tr(MessageId::OnboardToolsMcpLine2).to_string(),
        )),
        status_line(
            &app.tr(MessageId::OnboardToolsMcpMcpLabel),
            inventory.mcp.status,
            inventory.mcp.detail,
        ),
        status_line(
            &app.tr(MessageId::OnboardToolsMcpSkillsLabel),
            inventory.skills.status,
            inventory.skills.detail,
        ),
        status_line(
            &app.tr(MessageId::OnboardToolsMcpPluginsLabel),
            inventory.plugins.status,
            inventory.plugins.detail,
        ),
        Line::from(""),
        Line::from(Span::raw(
            app.tr(MessageId::OnboardToolsMcpLine3).to_string(),
        )),
        Line::from(Span::raw(
            app.tr(MessageId::OnboardToolsMcpLine4).to_string(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                app.tr(MessageId::OnboardToolsMcpFooterEnter).to_string(),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                app.tr(MessageId::OnboardToolsMcpFooterAction).to_string(),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]),
    ]
}

fn status_line(label: &str, status: InventoryStatus, detail: String) -> Line<'static> {
    let status_style = match status {
        InventoryStatus::Ready => Style::default()
            .fg(palette::STATUS_SUCCESS)
            .add_modifier(Modifier::BOLD),
        InventoryStatus::Optional => Style::default().fg(palette::TEXT_MUTED),
        InventoryStatus::NeedsAction => Style::default()
            .fg(palette::STATUS_WARNING)
            .add_modifier(Modifier::BOLD),
    };

    Line::from(vec![
        Span::styled("  ".to_string(), Style::default()),
        Span::styled(
            label.to_string(),
            Style::default().fg(palette::TEXT_PRIMARY),
        ),
        Span::raw(": "),
        Span::styled(status.as_str().to_string(), status_style),
        Span::raw(format!(" — {detail}")),
    ])
}

#[derive(Debug, Clone)]
struct Inventory {
    mcp: InventoryRow,
    skills: InventoryRow,
    plugins: InventoryRow,
}

impl Inventory {
    fn from_app(app: &App) -> Self {
        Self {
            mcp: mcp_inventory(app),
            skills: skills_inventory(app),
            plugins: plugins_inventory(app),
        }
    }
}

#[derive(Debug, Clone)]
struct InventoryRow {
    status: InventoryStatus,
    detail: String,
}

#[derive(Debug, Clone, Copy)]
enum InventoryStatus {
    Ready,
    Optional,
    NeedsAction,
}

impl InventoryStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Optional => "optional",
            Self::NeedsAction => "needs action",
        }
    }
}

fn mcp_inventory(app: &App) -> InventoryRow {
    if let Some(snapshot) = &app.mcp_snapshot {
        return mcp_snapshot_inventory(snapshot);
    }

    match crate::mcp::load_config_with_workspace(&app.mcp_config_path, &app.workspace) {
        Ok(cfg) => mcp_config_inventory(&app.mcp_config_path, app.mcp_config_path.exists(), &cfg),
        Err(_) => InventoryRow {
            status: InventoryStatus::NeedsAction,
            detail: format!(
                "config could not be read at {}; open /mcp to inspect it",
                display_path(&app.mcp_config_path)
            ),
        },
    }
}

fn mcp_snapshot_inventory(snapshot: &McpManagerSnapshot) -> InventoryRow {
    let total = snapshot.servers.len();
    if total == 0 {
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail: format!(
                "no servers configured at {}; add them later with /mcp",
                display_path(&snapshot.config_path)
            ),
        };
    }

    let enabled = snapshot
        .servers
        .iter()
        .filter(|server| server.enabled)
        .count();
    let disabled = total.saturating_sub(enabled);
    let connected = snapshot
        .servers
        .iter()
        .filter(|server| server.connected)
        .count();
    let needs_action = snapshot
        .servers
        .iter()
        .filter(|server| server.enabled && server.error.is_some())
        .count();
    let status = if needs_action > 0 {
        InventoryStatus::NeedsAction
    } else {
        InventoryStatus::Ready
    };
    let restart = if snapshot.restart_required {
        "; restart required"
    } else {
        ""
    };

    InventoryRow {
        status,
        detail: format!(
            "{total} configured, {enabled} enabled, {disabled} disabled, {connected} connected, {needs_action} need action{restart}; /mcp shows details"
        ),
    }
}

fn mcp_config_inventory(path: &std::path::Path, exists: bool, cfg: &McpConfig) -> InventoryRow {
    let total = cfg.servers.len();
    if total == 0 {
        let path_hint = if exists {
            format!("empty config at {}", display_path(path))
        } else {
            format!("no config at {}", display_path(path))
        };
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail: format!("{path_hint}; add optional servers later with /mcp"),
        };
    }

    let enabled = cfg
        .servers
        .values()
        .filter(|server| server.is_enabled())
        .count();
    let disabled = total.saturating_sub(enabled);

    InventoryRow {
        status: InventoryStatus::Ready,
        detail: format!(
            "{total} configured, {enabled} enabled, {disabled} disabled; health not probed here, use /mcp"
        ),
    }
}

fn skills_inventory(app: &App) -> InventoryRow {
    let count = app.cached_skills.len();
    let path = display_path(&app.skills_dir);
    if count == 0 {
        let detail = if app.skills_dir.exists() {
            format!("no skills discovered from primary path {path}; /skills can inspect")
        } else {
            format!("primary path missing at {path}; skills are optional")
        };
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail,
        };
    }

    InventoryRow {
        status: InventoryStatus::Ready,
        detail: format!("{count} discovered; /skills lists names and trust state"),
    }
}

fn plugins_inventory(app: &App) -> InventoryRow {
    let Some(plugin_dir) = plugin_dir_for(app) else {
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail: "plugin directory unavailable; set [tools].plugin_dir when needed".to_string(),
        };
    };
    let path = display_path(&plugin_dir);
    if !plugin_dir.exists() {
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail: format!("no plugin directory at {path}; plugins are optional"),
        };
    }

    let count = scan_plugin_dir(&plugin_dir).len();
    if count == 0 {
        return InventoryRow {
            status: InventoryStatus::Optional,
            detail: format!("no plugin scripts discovered in {path}; /plugins can inspect"),
        };
    }

    InventoryRow {
        status: InventoryStatus::Ready,
        detail: format!("{count} discovered in {path}; /plugins lists read-only metadata"),
    }
}

fn plugin_dir_for(app: &App) -> Option<PathBuf> {
    let config = match &app.config_path {
        Some(path) => {
            Config::load(Some(path.clone()), app.config_profile.as_deref()).unwrap_or_default()
        }
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
    use crate::localization::Locale;
    use crate::tui::app::TuiOptions;
    use tempfile::TempDir;

    fn test_app(
        workspace: &std::path::Path,
        config_path: Option<PathBuf>,
        mcp_config_path: PathBuf,
        skills_dir: PathBuf,
    ) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: workspace.to_path_buf(),
            config_path,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir,
            memory_path: workspace.join("memory.md"),
            notes_path: workspace.join("notes.txt"),
            mcp_config_path,
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = Locale::En;
        app
    }

    fn text(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .flat_map(|line| line.spans.into_iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn empty_inventory_is_optional_and_actionable() {
        let tmp = TempDir::new().expect("tempdir");
        let app = test_app(
            tmp.path(),
            None,
            tmp.path().join("mcp.json"),
            tmp.path().join("skills"),
        );

        let body = text(lines(&app));

        assert!(body.contains("MCP"));
        assert!(body.contains("Skills"));
        assert!(body.contains("Plugins"));
        assert!(body.contains("optional"));
        assert!(body.contains("/mcp"));
        assert!(body.contains("/skills"));
        assert!(body.contains("/plugins"));
    }

    #[test]
    fn configured_inventory_reports_counts_without_secret_details() {
        let tmp = TempDir::new().expect("tempdir");
        let mcp_path = tmp.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
              "servers": {
                "secret-server": {
                  "command": "run-secret-mcp",
                  "args": ["--token", "sk-mcp-secret"],
                  "env": {"API_KEY": "sk-env-secret"}
                }
              }
            }"#,
        )
        .expect("write mcp config");

        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("alpha")).expect("create skill dir");
        std::fs::write(
            skills_dir.join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: secret skill sk-skill-secret\n---\n",
        )
        .expect("write skill");

        let plugin_dir = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugin_dir).expect("create plugin dir");
        std::fs::write(
            plugin_dir.join("audit.sh"),
            "# name: audit\n# description: hides sk-plugin-secret\n# approval: required\n",
        )
        .expect("write plugin");

        let config_path = tmp.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                "[tools]\nplugin_dir = {}\n",
                toml::Value::String(plugin_dir.to_string_lossy().to_string())
            ),
        )
        .expect("write config");

        let app = test_app(tmp.path(), Some(config_path), mcp_path, skills_dir);
        let body = text(lines(&app));

        assert!(body.contains("1 configured"));
        assert!(body.contains("1 discovered"));
        assert!(!body.contains("run-secret-mcp"));
        assert!(!body.contains("sk-mcp-secret"));
        assert!(!body.contains("sk-env-secret"));
        assert!(!body.contains("sk-skill-secret"));
        assert!(!body.contains("sk-plugin-secret"));
        assert!(!body.contains("secret-server"));
        assert!(!body.contains("audit.sh"));
    }
}
