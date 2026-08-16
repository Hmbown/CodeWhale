//! Runtime status command.

use std::fmt::Write as _;
use std::path::Path;

use super::CommandResult;
use crate::compaction::estimate_input_tokens_conservative;
use crate::tui::app::App;
use crate::utils::{display_path, estimate_message_chars};

/// Show a compact runtime status report for the current TUI session.
pub fn status(app: &mut App) -> CommandResult {
    CommandResult::message(format_status(app))
}

fn format_status(app: &App) -> String {
    let mut out = String::new();
    let (context_used, context_max, context_percent) = context_usage(app);

    let _ = writeln!(out, "codewhale Status");
    let _ = writeln!(out, "===================");
    let _ = writeln!(out);
    push_row(&mut out, "Version:", env!("CARGO_PKG_VERSION"));
    push_row(
        &mut out,
        "Provider:",
        app.provider_identity_for_persistence(),
    );
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
    push_row(&mut out, "Safety:", safety_summary(app));
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
    push_row(&mut out, "Window source:", &context_window_source(app));
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
        "Session input:",
        &app.session.total_input_tokens.to_string(),
    );
    let session_cache =
        if app.session.total_cache_hit_tokens == 0 && app.session.total_cache_miss_tokens == 0 {
            "not reported".to_string()
        } else {
            format!(
                "{} hit / {} miss",
                app.session.total_cache_hit_tokens, app.session.total_cache_miss_tokens
            )
        };
    push_row(&mut out, "Session cache:", &session_cache);
    // The full, untrimmed session metrics strip (the footer sheds groups to
    // fit; here every group that has evidence is printed).
    let metrics = crate::tui::session_metrics::full_text(
        crate::tui::session_metrics::snapshot_from_app(app),
        app.ui_locale,
        crate::tui::color_compat::ascii_safe_enabled(),
    );
    let _ = writeln!(
        out,
        "  {}",
        crate::localization::tr(
            app.ui_locale,
            crate::localization::MessageId::SessionMetricsStatusLine
        )
        .replace("{metrics}", &metrics)
    );
    push_row(
        &mut out,
        "Session output:",
        &app.session.total_output_tokens.to_string(),
    );
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
    let tool_output_status =
        crate::tool_output_receipts::tool_output_status(&app.api_messages, &app.session_artifacts);
    push_row(
        &mut out,
        "Tool outputs:",
        &crate::tool_output_receipts::format_tool_output_status(&tool_output_status),
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
        app.approval_mode
            .permission_chip_label()
            .to_ascii_lowercase()
    )
}

fn safety_summary(app: &App) -> &'static str {
    let policy = crate::core::authority::sandbox_policy_for_turn(
        app.mode,
        app.approval_mode,
        app.configured_sandbox_mode.as_deref(),
        &app.workspace,
    );
    // The policy is the intent; `sandbox_backend` is what this platform can
    // actually enforce with. Default Linux (bubblewrap is opt-in) and all
    // Windows have none, and /status used to report "sandbox workspace-write"
    // while nothing was restricted (2026-08-04 audit). `doctor` has always
    // been honest about this; /status now agrees with it.
    let unenforced = app.sandbox_backend.is_none();
    match policy {
        crate::sandbox::SandboxPolicy::ReadOnly if unenforced => {
            "no OS sandbox on this platform (read-only requested, not enforced), network off"
        }
        crate::sandbox::SandboxPolicy::ReadOnly => "sandbox read-only, network off",
        crate::sandbox::SandboxPolicy::WorkspaceWrite { .. } if unenforced => {
            "no OS sandbox on this platform (workspace-write requested, not enforced), network on"
        }
        crate::sandbox::SandboxPolicy::WorkspaceWrite { .. } => {
            "sandbox workspace-write, network on"
        }
        crate::sandbox::SandboxPolicy::DangerFullAccess => "sandbox disabled, network unrestricted",
        crate::sandbox::SandboxPolicy::ExternalSandbox { .. } => {
            "external sandbox, network delegated to host"
        }
    }
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
    let max = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    let estimated =
        estimate_input_tokens_conservative(&app.api_messages, app.system_prompt.as_ref());
    let total_chars = estimate_message_chars(&app.api_messages);
    let used = estimated.max(total_chars / 4);
    let percent = ((used as f64 / f64::from(max)) * 100.0).clamp(0.0, 100.0);
    (used, max, percent)
}

/// Name where the effective context window came from and the exact key that
/// changes it.
///
/// #5134: `/status` printed the window as a bare number, so a user watching
/// auto-compaction fire at 128K on a 1M-capable model had no way to learn that
/// `context_window` exists, let alone which table it belongs on. The
/// provenance label alone is not enough — the actionable half is the key path.
fn context_window_source(app: &App) -> String {
    let source = app.active_context_window_source;
    let label = source.label();
    let Some(table) = app
        .api_provider
        .metadata()
        .map(|metadata| metadata.provider_config_key())
    else {
        return format!(
            "{label} (override: `context_window` on the active provider table in config.toml)"
        );
    };
    if source == crate::route_runtime::ContextWindowSource::Configured {
        format!("{label} — `[providers.{table}] context_window` in config.toml")
    } else {
        format!("{label} (override: `[providers.{table}] context_window` in config.toml)")
    }
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
    use crate::tui::app::{AppMode, TuiOptions};
    use crate::tui::history::HistoryCell;

    fn create_test_app(workspace: PathBuf) -> App {
        let options = TuiOptions {
            skills_dir: PathBuf::from("/tmp/test-skills"),
            ..crate::test_support::test_tui_options(workspace)
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
        assert!(msg.contains("codewhale Status"));
        assert!(msg.contains("Provider:"));
        assert!(msg.contains("Model:"));
        assert!(msg.contains("Directory:"));
        assert!(msg.contains("Permissions:"));
        assert!(msg.contains("Project docs:"));
        assert!(msg.contains("AGENTS.md"));
        assert!(msg.contains("Session:"));
        assert!(msg.contains("session-123"));
        assert!(msg.contains("Context window:"));
        assert!(msg.contains("Tool outputs:"));
        assert!(msg.contains("Cache hit/miss:"));
        assert!(msg.contains("70 hit / 30 miss"));
        assert!(msg.contains("Use /statusline to configure footer items."));
    }

    /// #5134: the number alone sends users to the issue tracker. `/status` has
    /// to name the provenance and the key that changes it, and it must name the
    /// table the user is actually on — not a generic placeholder.
    #[test]
    fn status_report_names_context_window_source_and_override_key() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.api_provider = ApiProvider::Moonshot;

        let msg = status(&mut app).message.expect("status message");

        let row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Window source:"))
            .expect("window source row");
        assert!(
            row.contains("[providers.moonshot] context_window"),
            "override key must be spelled for the active provider: {row}"
        );

        // A user override reads as a statement of fact, not as advice to set
        // something that is already set.
        app.active_context_window_source = crate::route_runtime::ContextWindowSource::Configured;
        let msg = status(&mut app).message.expect("status message");
        let row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Window source:"))
            .expect("window source row");
        assert!(row.contains("configured"), "{row}");
        assert!(!row.contains("override:"), "{row}");
    }

    #[test]
    fn status_report_keeps_exact_named_custom_provider() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.set_provider_identity(ApiProvider::Custom, "lm-studio");

        let msg = status(&mut app).message.expect("status message");

        let provider_row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Provider:"))
            .expect("provider row");
        assert_eq!(provider_row.split_whitespace().last(), Some("lm-studio"));
        assert_ne!(provider_row.split_whitespace().last(), Some("custom"));
    }

    #[test]
    fn status_report_surfaces_effective_safety_policy() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        // `/status` is honest about enforcement: on a platform with no OS
        // sandbox (e.g. Windows) it reports "<policy> requested, not enforced"
        // instead of the enforced string. The test must hold on both, so it
        // branches on the same signal `safety_summary` uses (`sandbox_backend`).
        let unenforced = app.sandbox_backend.is_none();

        app.mode = AppMode::Agent;
        let agent = format_status(&app);
        assert!(agent.contains("Safety:"));
        if unenforced {
            assert!(agent.contains("workspace-write requested, not enforced"));
        } else {
            assert!(agent.contains("sandbox workspace-write, network on"));
        }

        app.approval_mode = crate::tui::approval::ApprovalMode::Bypass;
        let full_access = format_status(&app);
        assert!(full_access.contains("sandbox disabled, network unrestricted"));

        app.configured_sandbox_mode = Some("workspace-write".to_string());
        let clamped = format_status(&app);
        if unenforced {
            assert!(clamped.contains("workspace-write requested, not enforced"));
        } else {
            assert!(clamped.contains("sandbox workspace-write, network on"));
        }

        app.mode = AppMode::Plan;
        let plan = format_status(&app);
        if unenforced {
            assert!(plan.contains("read-only requested, not enforced"));
        } else {
            assert!(plan.contains("sandbox read-only, network off"));
        }

        app.configured_sandbox_mode = None;
        app.mode = AppMode::Yolo;
        let yolo = format_status(&app);
        assert!(yolo.contains("sandbox disabled, network unrestricted"));
    }

    #[test]
    fn status_report_surfaces_large_tool_output_pressure() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        let raw = "RAW_STATUS_PRESSURE\n".repeat(2_000);
        app.api_messages.push(Message {
            role: "user".to_string(),
            content: vec![ContentBlock::ToolResult {
                tool_use_id: "call-big".to_string(),
                content: raw,
                is_error: None,
                content_blocks: None,
            }],
        });
        app.session_artifacts
            .push(crate::artifacts::ArtifactRecord {
                id: "art_call-big".to_string(),
                kind: crate::artifacts::ArtifactKind::ToolOutput,
                session_id: "session-123".to_string(),
                tool_call_id: "call-big".to_string(),
                tool_name: "exec_shell".to_string(),
                created_at: chrono::Utc::now(),
                byte_size: 24_000,
                preview: "large output".to_string(),
                storage_path: PathBuf::from("artifacts/art_call-big.txt"),
            });

        let result = status(&mut app);
        let msg = result.message.expect("status message");

        assert!(msg.contains("Tool outputs:"));
        assert!(msg.contains("raw over cap"));
        assert!(msg.contains("context pressure"));
        assert!(msg.contains("artifact"));
    }

    #[test]
    fn project_docs_reports_missing_docs() {
        let tmpdir = TempDir::new().expect("temp dir");
        assert_eq!(project_docs(tmpdir.path()), "not found");
    }
}
