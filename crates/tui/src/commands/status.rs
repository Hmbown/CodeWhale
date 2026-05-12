//! Runtime status command.

use std::fmt::Write as _;
use std::path::Path;

use super::CommandResult;
use crate::compaction::estimate_input_tokens_conservative;
use crate::models::{LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS, context_window_for_model};
use crate::tui::app::App;
use crate::utils::{display_path, estimate_message_chars};

/// Show a compact runtime status report for the current TUI session.
pub fn status(app: &mut App) -> CommandResult {
    CommandResult::message(format_status(app))
}

fn format_status(app: &App) -> String {
    let mut out = String::new();
    let (context_used, context_max, context_percent) = context_usage(app);

    let _ = writeln!(out, "DeepSeek TUI Status");
    let _ = writeln!(out, "===================");
    let _ = writeln!(out);
    push_row(&mut out, "Version:", env!("CARGO_PKG_VERSION"));
    push_row(&mut out, "Provider:", app.api_provider.as_str());
    push_row(
        &mut out,
        "Model:",
        &format!(
            "{} (reasoning {})",
            app.model_display_label(),
            app.reasoning_effort_display_label()
        ),
    );
    push_row(&mut out, "Directory:", &display_path(&app.workspace));
    push_row(&mut out, "Mode:", app.mode.label());
    push_row(&mut out, "Permissions:", &permission_summary(app));
    push_row(&mut out, "Project docs:", &project_docs(&app.workspace));
    push_row(
        &mut out,
        "Session:",
        app.current_session_id.as_deref().unwrap_or("not saved yet"),
    );
    push_row(
        &mut out,
        "MCP:",
        &format!("{} configured", app.mcp_configured_count),
    );
    push_row(&mut out, "Footer items:", &footer_items(app));
    let _ = writeln!(out);
    push_row(
        &mut out,
        "Context window:",
        &format!("{context_percent:.1}% used ({context_used} / {context_max} tokens)"),
    );
    push_row(
        &mut out,
        "Last API input:",
        &token_count(app.session.last_prompt_tokens),
    );
    push_row(
        &mut out,
        "Last API output:",
        &token_count(app.session.last_completion_tokens),
    );
    push_row(&mut out, "Cache hit/miss:", &cache_summary(app));
    push_row(
        &mut out,
        "Total tokens:",
        &app.session.total_tokens.to_string(),
    );
    push_row(
        &mut out,
        "Session cost:",
        &app.format_cost_amount_precise(app.session_cost_for_currency(app.cost_currency)),
    );
    push_row(
        &mut out,
        "Transcript:",
        &format!(
            "{} cells, {} API messages",
            app.history.len(),
            app.api_messages.len()
        ),
    );
    push_row(
        &mut out,
        "Rate limits:",
        "not available from provider telemetry",
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "Use /statusline to configure footer items.");

    out
}

fn push_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "  {label:<16} {value}");
}

fn permission_summary(app: &App) -> String {
    let trust = if app.trust_mode {
        "trusted workspace"
    } else {
        "workspace"
    };
    let shell = if app.allow_shell {
        "shell on"
    } else {
        "shell off"
    };
    format!(
        "{trust}, approvals {}, {shell}",
        app.approval_mode.label().to_ascii_lowercase()
    )
}

fn project_docs(workspace: &Path) -> String {
    let docs: Vec<&str> = ["AGENTS.md", "CLAUDE.md"]
        .into_iter()
        .filter(|name| workspace.join(name).is_file())
        .collect();
    if docs.is_empty() {
        "not found".to_string()
    } else {
        docs.join(", ")
    }
}

fn footer_items(app: &App) -> String {
    if app.status_items.is_empty() {
        return "none".to_string();
    }
    app.status_items
        .iter()
        .map(|item| item.key())
        .collect::<Vec<_>>()
        .join(", ")
}

fn context_usage(app: &App) -> (usize, u32, f64) {
    let max = context_window_for_model(&app.model).unwrap_or(LEGACY_DEEPSEEK_CONTEXT_WINDOW_TOKENS);
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    let total_chars = estimate_message_chars(&app.api_messages);
    let used = estimated.max(total_chars / 4);
    let percent = ((used as f64 / f64::from(max)) * 100.0).clamp(0.0, 100.0);
    (used, max, percent)
}

fn token_count(value: Option<u32>) -> String {
    value.map_or_else(|| "not reported".to_string(), |tokens| tokens.to_string())
}

fn cache_summary(app: &App) -> String {
    match (
        app.session.last_prompt_cache_hit_tokens,
        app.session.last_prompt_cache_miss_tokens,
    ) {
        (Some(hit), Some(miss)) => format!("{hit} hit / {miss} miss"),
        (Some(hit), None) => format!("{hit} hit / miss not reported"),
        (None, Some(miss)) => format!("hit not reported / {miss} miss"),
        (None, None) => "not reported".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::models::{ContentBlock, Message};
    use crate::tui::app::TuiOptions;
    use crate::tui::history::HistoryCell;

    fn create_test_app(workspace: PathBuf) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
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
        app.api_provider = ApiProvider::Deepseek;
        app
    }

    #[test]
    fn status_report_includes_runtime_fields() {
        let tmpdir = TempDir::new().expect("temp dir");
        std::fs::write(tmpdir.path().join("AGENTS.md"), "# Instructions").expect("write docs");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.current_session_id = Some("session-123".to_string());
        app.session.total_tokens = 1234;
        app.session.last_prompt_tokens = Some(100);
        app.session.last_completion_tokens = Some(25);
        app.session.last_prompt_cache_hit_tokens = Some(70);
        app.session.last_prompt_cache_miss_tokens = Some(30);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        });
        app.history.push(HistoryCell::User {
            content: "hello".to_string(),
        });

        let result = status(&mut app);
        let msg = result.message.expect("status message");
        assert!(msg.contains("DeepSeek TUI Status"));
        assert!(msg.contains("Provider:"));
        assert!(msg.contains("Model:"));
        assert!(msg.contains("Directory:"));
        assert!(msg.contains("Permissions:"));
        assert!(msg.contains("Project docs:"));
        assert!(msg.contains("AGENTS.md"));
        assert!(msg.contains("Session:"));
        assert!(msg.contains("session-123"));
        assert!(msg.contains("Context window:"));
        assert!(msg.contains("Cache hit/miss:"));
        assert!(msg.contains("70 hit / 30 miss"));
        assert!(msg.contains("Use /statusline to configure footer items."));
    }

    #[test]
    fn project_docs_reports_missing_docs() {
        let tmpdir = TempDir::new().expect("temp dir");
        assert_eq!(project_docs(tmpdir.path()), "not found");
    }
}
