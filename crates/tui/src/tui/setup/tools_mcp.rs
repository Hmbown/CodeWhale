//! Tools / MCP / skills / plugins setup inventory (#3407).
//!
//! Read-only discovery surface for the setup wizard. Classifies each surface as
//! `healthy` / `needs_config` / `off`, redacts secrets, and never spawns MCP
//! servers, installs skills, or runs plugins. Side-effectful bootstrap stays
//! behind explicit CLI/TUI commands listed in the on-ramp.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::localization::{Locale, MessageId, tr};
use crate::mcp::{McpConfig, McpManagerSnapshot, McpServerConfig, McpServerSnapshot};
use crate::tui::app::App;
use crate::tui::hotbar::actions::HotbarActionCategory;
use crate::utils::display_path;

/// Per-surface readiness vocabulary shared with setup summaries and doctor-like
/// copy. These never block first-run; they only describe optional power tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InventoryStatus {
    /// Configured and statically sound (or live-connected when a snapshot exists).
    Healthy,
    /// Present but incomplete/broken — needs user action outside first-run.
    NeedsConfig,
    /// Disabled or not configured. Friendly empty state, not an error.
    Off,
}

impl InventoryStatus {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::NeedsConfig => "needs_config",
            Self::Off => "off",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Off => 0,
            Self::Healthy => 1,
            Self::NeedsConfig => 2,
        }
    }

    fn worse(self, other: Self) -> Self {
        if other.rank() > self.rank() {
            other
        } else {
            self
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InventoryRow {
    status: InventoryStatus,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SetupToolsMcpFacts {
    pub(super) servers_result: String,
    pub(super) skills_result: String,
    pub(super) tools_result: String,
    pub(super) plugins_result: String,
    pub(super) hotbar_result: String,
    pub(super) result: String,
    pub(super) overall_status: InventoryStatus,
    pub(super) needs_action: bool,
    pub(super) mcp_path_display: String,
    pub(super) skills_path_display: String,
    pub(super) plugins_path_display: String,
}

impl Default for SetupToolsMcpFacts {
    fn default() -> Self {
        Self {
            servers_result: "MCP config not loaded".to_string(),
            skills_result: "skills dir not loaded".to_string(),
            tools_result: "tools dir not loaded".to_string(),
            plugins_result: "plugins dir not loaded".to_string(),
            hotbar_result: "hotbar source metadata not loaded".to_string(),
            result: "tools/MCP not loaded".to_string(),
            overall_status: InventoryStatus::Off,
            needs_action: false,
            mcp_path_display: String::new(),
            skills_path_display: String::new(),
            plugins_path_display: String::new(),
        }
    }
}

impl SetupToolsMcpFacts {
    pub(super) fn from_app_config(app: &App, config: &Config, codewhale_home: &Path) -> Self {
        let project_mcp_path = crate::mcp::workspace_mcp_config_path(&app.workspace);
        let mcp = mcp_inventory(app, &project_mcp_path);
        let skills = skills_inventory(app);
        let tools_dir = codewhale_home.join("tools");
        let tools = tools_dir_inventory(&tools_dir);
        let plugins = plugins_inventory(app, config, codewhale_home);
        let hotbar = hotbar_source_inventory(app);

        let overall = mcp
            .status
            .worse(skills.status)
            .worse(tools.status)
            .worse(plugins.status);
        // Only configured-but-broken surfaces need action. Empty/off is fine.
        let needs_action = matches!(mcp.status, InventoryStatus::NeedsConfig)
            || matches!(skills.status, InventoryStatus::NeedsConfig)
            || matches!(tools.status, InventoryStatus::NeedsConfig)
            || matches!(plugins.status, InventoryStatus::NeedsConfig);

        let mcp_path_display = display_path(&app.mcp_config_path);
        let skills_path_display = display_path(&app.skills_dir);
        let plugins_path_display = display_path(&plugins_dir_for(app, config, codewhale_home));

        let servers_result = format!("{} — {}", mcp.status.as_str(), mcp.detail);
        let skills_result = format!("{} — {}", skills.status.as_str(), skills.detail);
        let tools_result = format!("{} — {}", tools.status.as_str(), tools.detail);
        let plugins_result = format!("{} — {}", plugins.status.as_str(), plugins.detail);
        let hotbar_result = format!("{} — {}", hotbar.status.as_str(), hotbar.detail);

        let result = format!(
            "mcp={}, skills={}, tools={}, plugins={}, hotbar_sources={}, overall={}, mode=read_only_safe_probe",
            mcp.status.as_str(),
            skills.status.as_str(),
            tools.status.as_str(),
            plugins.status.as_str(),
            hotbar.detail,
            overall.as_str(),
        );

        Self {
            servers_result,
            skills_result,
            tools_result,
            plugins_result,
            hotbar_result,
            result,
            overall_status: overall,
            needs_action,
            mcp_path_display,
            skills_path_display,
            plugins_path_display,
        }
    }
}

pub(super) fn on_ramp_text(locale: Locale, facts: &SetupToolsMcpFacts) -> String {
    let base = tr(locale, MessageId::SetupToolsMcpOnRampText);
    base.replace("{mcp_result}", &facts.servers_result)
        .replace("{skills_result}", &facts.skills_result)
        .replace("{tools_result}", &facts.tools_result)
        .replace("{plugins_result}", &facts.plugins_result)
        .replace("{hotbar_result}", &facts.hotbar_result)
        .replace("{mcp_path}", &facts.mcp_path_display)
        .replace("{skills_path}", &facts.skills_path_display)
        .replace("{plugins_path}", &facts.plugins_path_display)
}

fn mcp_inventory(app: &App, project_mcp_path: &Path) -> InventoryRow {
    if let Some(snapshot) = app.mcp_snapshot.as_ref() {
        return mcp_snapshot_inventory(snapshot, &app.mcp_config_path, project_mcp_path);
    }

    match crate::mcp::load_config_with_workspace(&app.mcp_config_path, &app.workspace) {
        Ok(cfg) => mcp_config_inventory(&app.mcp_config_path, project_mcp_path, &cfg),
        Err(_) => InventoryRow {
            status: InventoryStatus::NeedsConfig,
            detail: format!(
                "config unreadable at {} (and project {}); open /mcp or run `codewhale doctor` — secrets not shown",
                display_path(&app.mcp_config_path),
                display_path(project_mcp_path)
            ),
        },
    }
}

fn mcp_path_presence(global: &Path, project: &Path) -> String {
    let global_state = if global.exists() {
        "global present"
    } else {
        "global missing"
    };
    let project_state = if project.exists() {
        "project present"
    } else {
        "project missing"
    };
    format!(
        "{global_state} at {}; {project_state} at {}",
        display_path(global),
        display_path(project)
    )
}

fn mcp_snapshot_inventory(
    snapshot: &McpManagerSnapshot,
    global_path: &Path,
    project_path: &Path,
) -> InventoryRow {
    let total = snapshot.servers.len();
    let paths = mcp_path_presence(global_path, project_path);
    if total == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "nothing configured yet ({paths}); optional — use /mcp or `codewhale mcp init` later"
            ),
        };
    }

    let mut healthy = 0usize;
    let mut needs_config = 0usize;
    let mut off = 0usize;
    let mut names_ok: Vec<&str> = Vec::new();
    let mut names_bad: Vec<&str> = Vec::new();
    let mut names_off: Vec<&str> = Vec::new();

    for server in &snapshot.servers {
        match classify_snapshot_server(server) {
            InventoryStatus::Healthy => {
                healthy += 1;
                if names_ok.len() < 4 {
                    names_ok.push(server.name.as_str());
                }
            }
            InventoryStatus::NeedsConfig => {
                needs_config += 1;
                if names_bad.len() < 4 {
                    names_bad.push(server.name.as_str());
                }
            }
            InventoryStatus::Off => {
                off += 1;
                if names_off.len() < 4 {
                    names_off.push(server.name.as_str());
                }
            }
        }
    }

    let status = if needs_config > 0 {
        InventoryStatus::NeedsConfig
    } else if healthy > 0 {
        InventoryStatus::Healthy
    } else {
        InventoryStatus::Off
    };

    let mut detail = format!(
        "{total} configured ({healthy} healthy, {needs_config} needs_config, {off} off; {paths})"
    );
    if !names_ok.is_empty() {
        detail.push_str(&format!("; healthy: {}", names_ok.join(", ")));
    }
    if !names_bad.is_empty() {
        detail.push_str(&format!("; needs_config: {}", names_bad.join(", ")));
    }
    if !names_off.is_empty() {
        detail.push_str(&format!("; off: {}", names_off.join(", ")));
    }
    if snapshot.restart_required {
        detail.push_str("; restart required for live tool list");
    }
    detail.push_str("; /mcp for details (commands/tokens never shown here)");
    InventoryRow { status, detail }
}

fn classify_snapshot_server(server: &McpServerSnapshot) -> InventoryStatus {
    if !server.enabled {
        return InventoryStatus::Off;
    }
    if server.connected {
        return InventoryStatus::Healthy;
    }
    match server.error.as_deref() {
        None => InventoryStatus::Healthy,
        Some("disabled") => InventoryStatus::Off,
        Some(_) => InventoryStatus::NeedsConfig,
    }
}

fn mcp_config_inventory(global: &Path, project: &Path, cfg: &McpConfig) -> InventoryRow {
    let total = cfg.servers.len();
    let paths = mcp_path_presence(global, project);
    if total == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "nothing configured yet ({paths}); optional — use /mcp or `codewhale mcp init` later"
            ),
        };
    }

    let mut healthy = 0usize;
    let mut needs_config = 0usize;
    let mut off = 0usize;
    let mut names_ok: Vec<&str> = Vec::new();
    let mut names_bad: Vec<&str> = Vec::new();
    let mut names_off: Vec<&str> = Vec::new();

    for (name, server) in &cfg.servers {
        match classify_config_server(server) {
            InventoryStatus::Healthy => {
                healthy += 1;
                if names_ok.len() < 4 {
                    names_ok.push(name.as_str());
                }
            }
            InventoryStatus::NeedsConfig => {
                needs_config += 1;
                if names_bad.len() < 4 {
                    names_bad.push(name.as_str());
                }
            }
            InventoryStatus::Off => {
                off += 1;
                if names_off.len() < 4 {
                    names_off.push(name.as_str());
                }
            }
        }
    }

    let status = if needs_config > 0 {
        InventoryStatus::NeedsConfig
    } else if healthy > 0 {
        InventoryStatus::Healthy
    } else {
        InventoryStatus::Off
    };

    let mut detail = format!(
        "{total} configured ({healthy} healthy, {needs_config} needs_config, {off} off; {paths}); static probe only — servers not started"
    );
    if !names_ok.is_empty() {
        detail.push_str(&format!("; healthy: {}", names_ok.join(", ")));
    }
    if !names_bad.is_empty() {
        detail.push_str(&format!("; needs_config: {}", names_bad.join(", ")));
    }
    if !names_off.is_empty() {
        detail.push_str(&format!("; off: {}", names_off.join(", ")));
    }
    detail.push_str("; /mcp or `codewhale doctor` for full checks");
    InventoryRow { status, detail }
}

/// Safe static probe aligned with `doctor_check_mcp_server` without spawning.
fn classify_config_server(server: &McpServerConfig) -> InventoryStatus {
    if !server.is_enabled() {
        return InventoryStatus::Off;
    }
    if server.command.is_none() && server.url.is_none() {
        return InventoryStatus::NeedsConfig;
    }
    if let Some(cmd) = server.command.as_deref() {
        if cmd.trim().is_empty() {
            return InventoryStatus::NeedsConfig;
        }
        let path = Path::new(cmd);
        let is_absolute = path.is_absolute() || cmd.starts_with('/');
        if is_absolute && !path.exists() {
            return InventoryStatus::NeedsConfig;
        }
    }
    // Env-backed bearer tokens: missing env is needs_config when URL-based.
    if server.url.is_some()
        && let Some(env_var) = server.bearer_token_env_var.as_deref()
        && !env_var.is_empty()
        && std::env::var_os(env_var).is_none()
    {
        return InventoryStatus::NeedsConfig;
    }
    InventoryStatus::Healthy
}

fn skills_inventory(app: &App) -> InventoryRow {
    let path = display_path(&app.skills_dir);
    let discovered = app.cached_skills.len();
    let dir_exists = app.skills_dir.exists();
    let dir_is_dir = app.skills_dir.is_dir();

    if !dir_exists {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "nothing configured yet (missing at {path}); optional — /skills or `codewhale setup --skills`"
            ),
        };
    }
    if !dir_is_dir {
        return InventoryRow {
            status: InventoryStatus::NeedsConfig,
            detail: format!("skills path exists but is not a directory at {path}"),
        };
    }

    // Count on-disk SKILL.md entries without executing anything.
    let on_disk = count_skill_dirs(&app.skills_dir);
    if discovered == 0 && on_disk == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "dir present at {path} with 0 skills; optional — install later via /skills install"
            ),
        };
    }

    InventoryRow {
        status: InventoryStatus::Healthy,
        detail: format!(
            "{discovered} discovered (hotbar skill sources), {on_disk} on disk at {path}; /skills lists names and trust"
        ),
    }
}

fn tools_dir_inventory(tools_dir: &Path) -> InventoryRow {
    let path = display_path(tools_dir);
    if !tools_dir.exists() {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "nothing configured yet (missing at {path}); optional — `codewhale setup --tools`"
            ),
        };
    }
    if !tools_dir.is_dir() {
        return InventoryRow {
            status: InventoryStatus::NeedsConfig,
            detail: format!("tools path exists but is not a directory at {path}"),
        };
    }
    let script_plugins = crate::tools::plugin::scan_plugin_dir(tools_dir).len();
    let entries = count_dir_entries(tools_dir);
    if entries == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!("empty tools dir at {path}; optional"),
        };
    }
    InventoryRow {
        status: InventoryStatus::Healthy,
        detail: format!(
            "{entries} entries, {script_plugins} script-plugin tools at {path}; not executed during setup"
        ),
    }
}

fn plugins_inventory(app: &App, config: &Config, codewhale_home: &Path) -> InventoryRow {
    let plugins_dir = plugins_dir_for(app, config, codewhale_home);
    let path = display_path(&plugins_dir);

    // Manifest-based plugins (plugin.toml) — prefer live registry when init'd.
    let (manifest_total, manifest_enabled) = if let Some(counts) =
        crate::plugins::try_with_registry(|registry| {
            let list = registry.list();
            let total = list.len();
            let enabled = list.iter().filter(|(_, p)| p.enabled).count();
            (total, enabled)
        }) {
        counts
    } else {
        count_manifest_plugins(&plugins_dir)
    };

    // Script plugins under [tools].plugin_dir (distinct from slash commands;
    // Hotbar Plugin source remains deferred/exploratory).
    let script_dir = config
        .tools
        .as_ref()
        .and_then(|tools| tools.plugin_dir.as_ref())
        .map(PathBuf::from)
        .filter(|p| p.as_path() != plugins_dir.as_path());
    let script_count = script_dir
        .as_ref()
        .filter(|p| p.is_dir())
        .map(|p| crate::tools::plugin::scan_plugin_dir(p).len())
        .unwrap_or(0);

    if !plugins_dir.exists() && manifest_total == 0 && script_count == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "nothing configured yet (missing at {path}); optional — `codewhale setup --plugins`; plugin commands stay distinct from slash and are deferred on Hotbar"
            ),
        };
    }

    if plugins_dir.exists() && !plugins_dir.is_dir() {
        return InventoryRow {
            status: InventoryStatus::NeedsConfig,
            detail: format!("plugins path exists but is not a directory at {path}"),
        };
    }

    if manifest_total == 0 && script_count == 0 {
        return InventoryRow {
            status: InventoryStatus::Off,
            detail: format!(
                "dir present at {path} with 0 plugins; plugin commands not enumerated as slash commands (Hotbar plugin source deferred)"
            ),
        };
    }

    let disabled = manifest_total.saturating_sub(manifest_enabled);
    InventoryRow {
        status: InventoryStatus::Healthy,
        detail: format!(
            "{manifest_total} manifest plugins ({manifest_enabled} enabled, {disabled} off), {script_count} script tools; path {path}; /plugin inspects metadata only — never auto-run"
        ),
    }
}

fn plugins_dir_for(_app: &App, _config: &Config, codewhale_home: &Path) -> PathBuf {
    codewhale_home.join("plugins")
}

fn hotbar_source_inventory(app: &App) -> InventoryRow {
    // Reuse the same Hotbar action registry the setup Hotbar step and command
    // palette already share — do not re-discover MCP/skills here.
    let mut mcp = 0usize;
    let mut skill = 0usize;
    let mut plugin = 0usize;
    let mut slash = 0usize;
    for action in app.hotbar_actions.iter() {
        match action.category() {
            c if c == HotbarActionCategory::Mcp.as_str() => mcp += 1,
            c if c == HotbarActionCategory::Skill.as_str() => skill += 1,
            c if c == HotbarActionCategory::Plugin.as_str() => plugin += 1,
            c if c == HotbarActionCategory::Slash.as_str() => slash += 1,
            _ => {}
        }
    }
    // Plugin source is deferred by design (#3399) — zero dispatchable plugin
    // actions is healthy, not a failure.
    let status = if mcp > 0 || skill > 0 {
        InventoryStatus::Healthy
    } else {
        InventoryStatus::Off
    };
    InventoryRow {
        status,
        detail: format!(
            "shared adapters: mcp_actions={mcp}, skill_actions={skill}, plugin_actions={plugin} (deferred), slash_actions={slash}"
        ),
    }
}

fn count_dir_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy() != ".DS_Store")
                .count()
        })
        .unwrap_or(0)
}

fn count_skill_dirs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().join("SKILL.md").is_file())
                .count()
        })
        .unwrap_or(0)
}

fn count_manifest_plugins(dir: &Path) -> (usize, usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let mut total = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.join("plugin.toml").is_file() {
            total += 1;
        }
    }
    // Without overrides store, treat discovered manifests as enabled by default
    // (matches discovery: enabled = !builtin for user plugins).
    (total, total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::mcp::{McpDiscoveredItem, McpManagerSnapshot, McpServerSnapshot};
    use crate::tui::app::TuiOptions;
    use crate::tui::hotbar::actions::HotbarActionRegistry;
    use tempfile::TempDir;

    fn test_app(
        workspace: &Path,
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
            skills_dir: skills_dir.clone(),
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
        // App::new re-resolves skills via global/workspace discovery; pin the
        // hermetic test path and empty cache so host ~/.agents/skills cannot
        // leak into inventory assertions.
        app.skills_dir = skills_dir;
        app.cached_skills.clear();
        app.hotbar_actions = HotbarActionRegistry::with_builtins();
        app
    }

    #[test]
    fn empty_inventory_is_off_not_error() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let app = test_app(
            tmp.path(),
            None,
            tmp.path().join("mcp.json"),
            tmp.path().join("skills"),
        );

        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);

        assert!(
            facts.servers_result.contains("off"),
            "empty MCP should be off: {}",
            facts.servers_result
        );
        assert!(
            facts.skills_result.contains("off"),
            "missing skills dir should be off: {}",
            facts.skills_result
        );
        assert!(
            facts.plugins_result.contains("off"),
            "missing plugins should be off: {}",
            facts.plugins_result
        );
        assert!(
            !facts.needs_action,
            "empty optional inventory must not block"
        );
        assert_eq!(facts.overall_status, InventoryStatus::Off);
        assert!(facts.result.contains("mode=read_only_safe_probe"));
        assert!(facts.servers_result.contains("/mcp") || facts.servers_result.contains("optional"));
    }

    #[test]
    fn configured_mcp_reports_healthy_without_secret_details() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("cw-home");
        std::fs::create_dir_all(&home).expect("home");
        let mcp_path = tmp.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
              "servers": {
                "docs": {
                  "command": "npx",
                  "args": ["-y", "secret-mcp-package"],
                  "env": {"API_KEY": "sk-mcp-secret-token"},
                  "headers": {"Authorization": "Bearer sk-header-secret"}
                }
              }
            }"#,
        )
        .expect("write mcp");

        let skills_dir = tmp.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("alpha")).expect("skill dir");
        std::fs::write(
            skills_dir.join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: hides sk-skill-secret\n---\nbody\n",
        )
        .expect("skill");

        let plugins_dir = home.join("plugins");
        std::fs::create_dir_all(plugins_dir.join("demo")).expect("plugin");
        std::fs::write(
            plugins_dir.join("demo").join("plugin.toml"),
            "[plugin]\nname = \"demo\"\ndescription = \"hides sk-plugin-secret\"\n",
        )
        .expect("manifest");

        let mut app = test_app(tmp.path(), None, mcp_path, skills_dir);
        // Simulate the same skill registration Hotbar uses at startup.
        app.cached_skills = vec![("alpha".into(), "alpha skill".into())];
        app.hotbar_actions = HotbarActionRegistry::with_builtins();
        app.hotbar_actions.register_skills(&app.cached_skills);

        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);

        assert!(
            facts.servers_result.contains("healthy"),
            "configured MCP should be healthy: {}",
            facts.servers_result
        );
        assert!(facts.servers_result.contains("docs"));
        assert!(
            facts.skills_result.contains("healthy"),
            "installed skills: {}",
            facts.skills_result
        );
        assert!(
            facts.plugins_result.contains("healthy"),
            "manifest plugins: {}",
            facts.plugins_result
        );
        assert!(facts.hotbar_result.contains("skill_actions=1"));
        assert!(!facts.needs_action);

        // Redaction: never leak tokens, env values, or full command args.
        let blob = format!(
            "{} {} {} {} {}",
            facts.servers_result,
            facts.skills_result,
            facts.plugins_result,
            facts.hotbar_result,
            facts.result
        );
        assert!(!blob.contains("sk-mcp-secret-token"));
        assert!(!blob.contains("sk-header-secret"));
        assert!(!blob.contains("sk-skill-secret"));
        assert!(!blob.contains("sk-plugin-secret"));
        assert!(!blob.contains("secret-mcp-package"));
        assert!(!blob.contains("API_KEY"));
        assert!(!blob.contains("Bearer"));
    }

    #[test]
    fn failed_mcp_reports_needs_config() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let mcp_path = tmp.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{
              "servers": {
                "broken": {
                  "command": "/definitely/missing/mcp-server-binary",
                  "args": ["--token", "sk-should-not-leak"]
                },
                "off-server": {
                  "command": "npx",
                  "enabled": false
                }
              }
            }"#,
        )
        .expect("write mcp");

        let app = test_app(
            tmp.path(),
            None,
            mcp_path,
            tmp.path().join("skills-missing"),
        );
        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);

        assert!(
            facts.servers_result.contains("needs_config"),
            "broken absolute command should needs_config: {}",
            facts.servers_result
        );
        assert!(facts.servers_result.contains("broken"));
        assert!(facts.servers_result.contains("off"));
        assert!(facts.needs_action);
        assert!(!facts.servers_result.contains("sk-should-not-leak"));
        assert!(!facts.result.contains("sk-should-not-leak"));
    }

    #[test]
    fn live_snapshot_failed_server_is_needs_config() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let mut app = test_app(
            tmp.path(),
            None,
            tmp.path().join("mcp.json"),
            tmp.path().join("skills"),
        );
        app.mcp_snapshot = Some(McpManagerSnapshot {
            config_path: tmp.path().join("mcp.json"),
            config_exists: true,
            restart_required: false,
            servers: vec![
                McpServerSnapshot {
                    name: "ok".into(),
                    enabled: true,
                    required: false,
                    transport: "stdio".into(),
                    command_or_url: "npx secret-should-not-appear".into(),
                    connect_timeout: 10,
                    execute_timeout: 10,
                    read_timeout: 10,
                    connected: true,
                    error: None,
                    tools: vec![McpDiscoveredItem {
                        name: "tool_a".into(),
                        model_name: "mcp_ok_tool_a".into(),
                        description: Some("desc".into()),
                    }],
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
                McpServerSnapshot {
                    name: "bad".into(),
                    enabled: true,
                    required: false,
                    transport: "stdio".into(),
                    command_or_url: "run --token sk-live-secret".into(),
                    connect_timeout: 10,
                    execute_timeout: 10,
                    read_timeout: 10,
                    connected: false,
                    error: Some("spawn failed: connection refused".into()),
                    tools: Vec::new(),
                    resources: Vec::new(),
                    prompts: Vec::new(),
                },
            ],
        });

        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);
        assert!(facts.servers_result.contains("needs_config"));
        assert!(facts.servers_result.contains("bad"));
        assert!(facts.servers_result.contains("ok"));
        assert!(facts.needs_action);
        assert!(!facts.servers_result.contains("sk-live-secret"));
        assert!(!facts.servers_result.contains("secret-should-not-appear"));
        // Error detail text may mention connection refused but not secrets.
        assert!(!facts.servers_result.contains("spawn failed"));
    }

    #[test]
    fn missing_skills_dir_is_off_not_needs_config() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let missing = tmp.path().join("no-such-skills");
        let app = test_app(tmp.path(), None, tmp.path().join("mcp.json"), missing);
        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);
        assert!(facts.skills_result.starts_with("off"));
        assert!(!facts.skills_result.contains("needs_config"));
    }

    #[test]
    fn skills_path_not_directory_is_needs_config() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let skills_file = tmp.path().join("skills-as-file");
        std::fs::write(&skills_file, "not a dir").expect("file");
        let app = test_app(tmp.path(), None, tmp.path().join("mcp.json"), skills_file);
        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);
        assert!(facts.skills_result.contains("needs_config"));
        assert!(facts.needs_action);
    }

    #[test]
    fn plugin_unavailable_is_off_with_actionable_hint() {
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        // No plugins dir under home.
        let app = test_app(
            tmp.path(),
            None,
            tmp.path().join("mcp.json"),
            tmp.path().join("skills"),
        );
        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);
        assert!(
            facts.plugins_result.contains("off"),
            "{}",
            facts.plugins_result
        );
        assert!(
            facts.plugins_result.contains("optional")
                || facts.plugins_result.contains("setup --plugins")
                || facts.plugins_result.contains("deferred"),
            "actionable empty-plugin copy: {}",
            facts.plugins_result
        );
    }

    #[test]
    fn on_ramp_text_mentions_safe_commands_and_redacts() {
        let facts = SetupToolsMcpFacts {
            servers_result: "off — nothing configured".into(),
            skills_result: "off — missing".into(),
            tools_result: "off — missing".into(),
            plugins_result: "off — missing".into(),
            hotbar_result: "off — shared adapters: mcp_actions=0".into(),
            result: "overall=off".into(),
            overall_status: InventoryStatus::Off,
            needs_action: false,
            mcp_path_display: "~/.codewhale/mcp.json".into(),
            skills_path_display: "~/.codewhale/skills".into(),
            plugins_path_display: "~/.codewhale/plugins".into(),
        };
        let text = on_ramp_text(Locale::En, &facts);
        assert!(text.contains("codewhale mcp init") || text.contains("/mcp"));
        assert!(text.contains("/skills") || text.contains("setup --skills"));
        assert!(text.contains("does not") || text.contains("never") || text.contains("not run"));
        assert!(text.contains("~/.codewhale/mcp.json"));
        assert!(!text.contains("sk-"));
    }

    #[test]
    fn redacted_result_summary_omits_paths_with_home_secrets() {
        // result summary uses status tokens only — no raw env secrets.
        let tmp = TempDir::new().expect("tempdir");
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).expect("home");
        let mcp_path = tmp.path().join("mcp.json");
        std::fs::write(
            &mcp_path,
            r#"{"servers":{"s":{"command":"npx","env":{"TOKEN":"sk-result-secret"}}}}"#,
        )
        .expect("mcp");
        let app = test_app(tmp.path(), None, mcp_path, tmp.path().join("skills"));
        let facts = SetupToolsMcpFacts::from_app_config(&app, &Config::default(), &home);
        assert!(!facts.result.contains("sk-result-secret"));
        assert!(facts.result.contains("mcp="));
        assert!(facts.result.contains("mode=read_only_safe_probe"));
    }
}
