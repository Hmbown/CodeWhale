//! Read-only harness posture status command (#2693).

use std::fmt::Write as _;
use std::path::Path;

use codewhale_config::{
    HarnessCompactionStrategy, HarnessPostureKind, HarnessSafetyPosture, HarnessSource,
    HarnessToolSurface,
};

use crate::commands::CommandResult;
use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::MessageId;
use crate::tui::app::App;
use crate::utils::display_path;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "harness",
    aliases: &[],
    usage: "/harness",
    description_id: MessageId::CmdHarnessDescription,
};

pub(in crate::commands) struct HarnessCmd;

impl RegisterCommand for HarnessCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, _arg: Option<&str>) -> CommandResult {
        CommandResult::message(format_harness_status(app))
    }
}

pub(crate) fn format_harness_status(app: &App) -> String {
    let resolution = app.harness_resolution();
    let posture = &resolution.posture;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Harness profile (preview — does not change runtime behavior yet)"
    );
    let _ = writeln!(out);
    push_row(&mut out, "Provider", app.active_provider_route());
    push_row(&mut out, "Model", &app.model_display_label());
    let _ = writeln!(
        out,
        "  {:<12} {}            (source: {})",
        "Posture:",
        posture_kind_label(posture.kind),
        harness_source_label(resolution.source)
    );
    let _ = writeln!(
        out,
        "             max_subagents={}  compaction={}  tools={}  safety={}",
        posture.max_subagents,
        compaction_strategy_label(posture.compaction_strategy),
        tool_surface_label(posture.tool_surface),
        safety_posture_label(posture.safety_posture)
    );
    push_row(&mut out, "Repo law", &format_repo_law(app));

    out
}

fn push_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "  {label:<12} {value}");
}

fn format_repo_law(app: &App) -> String {
    match crate::project_context::discover_repo_constitution_path(&app.workspace) {
        Some(path) => repo_law_display_path(&app.workspace, &path),
        None => "none".to_string(),
    }
}

fn repo_law_display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|_| display_path(path))
}

fn posture_kind_label(kind: HarnessPostureKind) -> &'static str {
    match kind {
        HarnessPostureKind::Standard => "standard",
        HarnessPostureKind::CacheHeavy => "cache-heavy",
        HarnessPostureKind::Lean => "lean",
        HarnessPostureKind::Custom => "custom",
    }
}

fn harness_source_label(source: HarnessSource) -> &'static str {
    match source {
        HarnessSource::UserProfile => "user config",
        HarnessSource::BuiltInSeed => "built-in seed",
        HarnessSource::Default => "default (no match)",
    }
}

fn compaction_strategy_label(strategy: HarnessCompactionStrategy) -> &'static str {
    match strategy {
        HarnessCompactionStrategy::Default => "default",
        HarnessCompactionStrategy::PrefixCache => "prefix-cache",
        HarnessCompactionStrategy::Aggressive => "aggressive",
    }
}

fn tool_surface_label(surface: HarnessToolSurface) -> &'static str {
    match surface {
        HarnessToolSurface::Full => "full",
        HarnessToolSurface::ReadOnly => "read-only",
        HarnessToolSurface::Auto => "auto",
    }
}

fn safety_posture_label(posture: HarnessSafetyPosture) -> &'static str {
    match posture {
        HarnessSafetyPosture::Standard => "standard",
        HarnessSafetyPosture::Strict => "strict",
        HarnessSafetyPosture::Permissive => "permissive",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::tui::app::TuiOptions;

    fn create_test_app(workspace: PathBuf, model: &str, provider: ApiProvider) -> App {
        let options = TuiOptions {
            model: model.to_string(),
            workspace,
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("/tmp/test-skills"),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.api_provider = provider;
        app.model = model.to_string();
        app.auto_model = false;
        app
    }

    #[test]
    fn harness_command_renders_cache_heavy_with_source() {
        let tmpdir = TempDir::new().expect("temp dir");
        let app = create_test_app(
            tmpdir.path().to_path_buf(),
            "deepseek-v4-pro",
            ApiProvider::Deepseek,
        );
        let output = format_harness_status(&app);

        assert!(output.contains("does not change runtime behavior yet"));
        assert!(output.contains("Provider"));
        assert!(output.contains("deepseek"));
        assert!(output.contains("cache-heavy"));
        assert!(output.contains("built-in seed"));
    }

    #[test]
    fn harness_command_renders_standard_default() {
        let tmpdir = TempDir::new().expect("temp dir");
        let app = create_test_app(tmpdir.path().to_path_buf(), "gpt-x", ApiProvider::Openai);
        let output = format_harness_status(&app);

        assert!(output.contains("standard"));
        assert!(output.contains("default (no match)"));
    }

    #[test]
    fn harness_command_shows_repo_law_when_constitution_present() {
        let tmpdir = TempDir::new().expect("temp dir");
        let constitution_dir = tmpdir.path().join(".codewhale");
        std::fs::create_dir_all(&constitution_dir).expect("mkdir .codewhale");
        std::fs::write(
            constitution_dir.join("constitution.json"),
            r#"{"authority":["AGENTS.md"]}"#,
        )
        .expect("write constitution");

        let app = create_test_app(
            tmpdir.path().to_path_buf(),
            "deepseek-v4-pro",
            ApiProvider::Deepseek,
        );
        let output = format_harness_status(&app);
        assert!(output.contains("Repo law"));
        assert!(output.contains(".codewhale"));
        assert!(output.contains("constitution.json"));

        let missing = create_test_app(
            TempDir::new().expect("temp dir").path().to_path_buf(),
            "deepseek-v4-pro",
            ApiProvider::Deepseek,
        );
        let missing_output = format_harness_status(&missing);
        assert!(missing_output.contains("Repo law"));
        assert!(missing_output.contains("none"));
    }

    #[test]
    fn harness_status_is_read_only() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(
            tmpdir.path().to_path_buf(),
            "deepseek-v4-pro",
            ApiProvider::Deepseek,
        );
        let before_profiles = app.harness_profiles.clone();
        let before_provider = app.api_provider;
        let before_model = app.model.clone();

        let result = HarnessCmd::execute(&mut app, None);
        assert!(result.message.is_some());

        assert_eq!(app.harness_profiles, before_profiles);
        assert_eq!(app.api_provider, before_provider);
        assert_eq!(app.model, before_model);
    }
}
