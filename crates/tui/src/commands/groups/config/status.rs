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

/// Row label column, in columns. Widest label is `Context window:` (15);
/// the tail space in [`push_row`] makes the value column start at 19, which
/// is where the localized `Session metrics:` line already lands.
const LABEL_WIDTH: usize = 16;

fn format_status(app: &App) -> String {
    let mut out = String::new();
    let (context_used, context_max, context_percent) = context_usage(app);

    // A transcript cell has no ink and no rules, so the only grouping mark
    // available is a blank row. It is spent on the two group boundaries and
    // nowhere else: standing facts about the route and the machine first,
    // then everything that accumulates as the session runs.
    let _ = writeln!(out, "codewhale {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out);

    push_row(&mut out, "Route:", &route_summary(app));
    push_row(&mut out, "Directory:", &display_path(&app.workspace));
    push_row(&mut out, "Project docs:", &project_docs(&app.workspace));
    push_row(&mut out, "Mode:", &posture_summary(app));
    push_row(&mut out, "Safety:", safety_summary(app));
    push_row(
        &mut out,
        "MCP:",
        &format!("{} configured", app.mcp_configured_count),
    );
    let _ = writeln!(out);

    push_row(
        &mut out,
        "Context window:",
        &format!("{context_percent:.1}% used ({context_used} / {context_max} tokens)"),
    );
    push_row(
        &mut out,
        "Window source:",
        &context_window_source(app).display_label(),
    );
    if let Some(key) = context_window_override_key(app) {
        push_row(&mut out, "Window override:", &key);
    }
    push_row(&mut out, "Session:", &session_summary(app));
    push_row(&mut out, "Session tokens:", &session_tokens(app));
    push_row(
        &mut out,
        "Session cost:",
        &app.format_cost_amount_precise(app.session_cost_for_currency(app.cost_currency)),
    );
    // The full, untrimmed session metrics strip (the footer sheds groups to
    // fit; here every group that has evidence is printed). This is the one
    // localized label in the report, so it keeps its own writeln rather than
    // being re-spelled in English by `push_row`.
    let snapshot = crate::tui::session_metrics::snapshot_from_app(app);
    if !snapshot.is_empty() {
        let metrics = crate::tui::session_metrics::full_text(
            snapshot,
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
    }
    let tool_output_status =
        crate::tool_output_receipts::tool_output_status(&app.api_messages, &app.session_artifacts);
    push_row(
        &mut out,
        "Tool outputs:",
        &crate::tool_output_receipts::format_tool_output_status(&tool_output_status),
    );
    let _ = writeln!(out);
    // Two whole fields left this report rather than being printed at the same
    // weight as everything else: the per-turn token ledger, which `/tokens`
    // already prints in full, and the list of enabled footer item keys, which
    // is `/statusline`'s own subject. The pointer costs one row; they cost
    // seven.
    let _ = writeln!(
        out,
        "  Per-turn tokens: /tokens · Footer items: /statusline"
    );

    out
}

/// Provider, model, and effort as one lockup, matching the header rail.
///
/// These were three rows (`Provider:`, `Model:` with the effort parenthesised)
/// for one fact — which route is this turn going to. The header already joins
/// them with a middle dot; `/status` now agrees with it.
fn route_summary(app: &App) -> String {
    format!(
        "{} · {} · reasoning {}",
        app.provider_identity_for_persistence(),
        app.model_display_label(),
        app.reasoning_effort_display_label()
    )
}

/// Mode and the permissions that qualify it, as one statement of posture.
fn posture_summary(app: &App) -> String {
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
        "{} · approvals {} · {shell} · {trust}",
        app.mode.label(),
        app.approval_mode
            .permission_chip_label()
            .to_ascii_lowercase()
    )
}

/// Session identity and the size of the conversation it names.
fn session_summary(app: &App) -> String {
    format!(
        "{} · {} cells · {} API messages",
        app.current_session_id.as_deref().unwrap_or("not saved yet"),
        app.history.len(),
        app.api_messages.len()
    )
}

/// Cumulative token ledger on one row.
///
/// The session input/output split and the cumulative cache totals live only
/// here; the per-turn figures they used to sit beside are `/tokens`.
fn session_tokens(app: &App) -> String {
    let cache =
        if app.session.total_cache_hit_tokens == 0 && app.session.total_cache_miss_tokens == 0 {
            "cache not reported".to_string()
        } else {
            format!(
                "cache {} hit / {} miss",
                app.session.total_cache_hit_tokens, app.session.total_cache_miss_tokens
            )
        };
    format!(
        "{} in · {} out · {} total · {cache}",
        app.session.total_input_tokens, app.session.total_output_tokens, app.session.total_tokens
    )
}

fn push_row(out: &mut String, label: &str, value: &str) {
    let _ = writeln!(out, "  {label:<LABEL_WIDTH$} {value}");
}

fn safety_summary(app: &App) -> &'static str {
    let policy = crate::core::authority::sandbox_policy_for_turn(
        app.mode,
        app.approval_mode,
        app.configured_sandbox_mode.as_deref(),
        &app.workspace,
        crate::core::authority::SandboxNetworkAccess::from_config(app.configured_sandbox_network),
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
        // Read the flag rather than assuming it. Workspace-write defaults to
        // network-restricted, so a hardcoded "network on" here named a
        // boundary the policy does not grant.
        crate::sandbox::SandboxPolicy::WorkspaceWrite { network_access, .. } if unenforced => {
            if network_access {
                "no OS sandbox on this platform (workspace-write requested, not enforced), network on"
            } else {
                "no OS sandbox on this platform (workspace-write requested, not enforced), network requested off, not enforced"
            }
        }
        crate::sandbox::SandboxPolicy::WorkspaceWrite { network_access, .. } => {
            if network_access {
                "sandbox workspace-write, network on"
            } else {
                "sandbox workspace-write, network off"
            }
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
        "no project docs".to_string()
    } else {
        docs.join(", ")
    }
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

/// Where the effective context window came from.
///
/// #5134: `/status` printed the window as a bare number, so a user watching
/// auto-compaction fire at 128K on a 1M-capable model had no way to learn that
/// `context_window` exists, let alone which table it belongs on. The
/// provenance label alone is not enough — the actionable half is the key path,
/// which now gets its own aligned row rather than a parenthesis that wrapped
/// the provenance off the end of the line.
fn context_window_source(app: &App) -> crate::route_runtime::ContextWindowSource {
    app.active_context_window_source
}

/// The exact key that changes the window, or `None` when the user already set
/// it and the row would be naming a key they have already used.
fn context_window_override_key(app: &App) -> Option<String> {
    if app.active_context_window_source == crate::route_runtime::ContextWindowSource::Configured {
        return None;
    }
    let table = app
        .api_provider
        .metadata()
        .map(|metadata| metadata.provider_config_key());
    Some(match table {
        Some(table) => format!("[providers.{table}] context_window in config.toml"),
        None => "`context_window` on the active provider table in config.toml".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use crate::models::Role;
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
            role: Role::User,
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
        assert!(msg.starts_with(&format!("codewhale {}", env!("CARGO_PKG_VERSION"))));
        assert!(msg.contains("Route:"));
        assert!(msg.contains("Directory:"));
        assert!(msg.contains("AGENTS.md"));
        assert!(msg.contains("Mode:"));
        assert!(msg.contains("approvals"));
        assert!(msg.contains("Session:"));
        assert!(msg.contains("session-123"));
        assert!(msg.contains("Context window:"));
        assert!(msg.contains("Tool outputs:"));
        assert!(msg.contains("Session tokens:"));
        assert!(msg.contains("/tokens"));
        assert!(msg.contains("/statusline"));
    }

    /// Every row has to earn its place in a 24-row terminal. The report used
    /// to run 31 lines, so at 80x24 — where the transcript viewport is 18
    /// rows — a user who typed `/status` landed on the *tail*: the version,
    /// route, directory, mode and sandbox rows had already scrolled off, and
    /// what remained on screen was five "not reported" rows and a `$0.0000`.
    ///
    /// A fresh session is 18 rows, not 17: `Window override:` is present
    /// unless the value is already configured. That matches the viewport
    /// height, so the title still scrolls off once `/status` occupies a
    /// history cell.
    #[test]
    fn status_report_fits_a_short_terminal() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        let msg = status(&mut app).message.expect("status message");
        let rows = msg.lines().count();
        assert!(
            msg.contains("Window override:"),
            "fresh session keeps the override row: {msg}"
        );
        assert_eq!(
            rows, 18,
            "fresh session is 18 rows with Window override present, got {rows} rows:\n{msg}"
        );
    }

    /// `Rate limits:` was a `push_row` of a string literal — it could never
    /// report anything but "not available from provider telemetry". A row
    /// that cannot say anything cannot inform, and it cost a row on every
    /// terminal forever.
    #[test]
    fn status_report_drops_the_row_that_could_never_say_anything() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        let msg = status(&mut app).message.expect("status message");
        assert!(!msg.contains("Rate limits"), "{msg}");
        assert!(
            !msg.contains("not available from provider telemetry"),
            "{msg}"
        );
    }

    /// The per-turn ledger is `/tokens`' whole subject and `/status` printed
    /// six rows of it. Shedding the field beats printing it at the same
    /// weight as the sandbox policy — but only if the report says where it
    /// went, and only if the two facts that live nowhere else (the
    /// cumulative in/out split and the cumulative cache totals) survive.
    #[test]
    fn status_report_sheds_the_per_turn_ledger_and_names_where_it_went() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.session.total_input_tokens = 900;
        app.session.total_output_tokens = 120;
        app.session.total_tokens = 1020;
        app.session.total_cache_hit_tokens = 700;
        app.session.total_cache_miss_tokens = 200;
        app.session.last_prompt_tokens = Some(100);

        let msg = status(&mut app).message.expect("status message");

        for shed in [
            "Last API input:",
            "Last API output:",
            "Cache hit/miss:",
            "Session input:",
            "Session output:",
            "Total tokens:",
            "Session cache:",
        ] {
            assert!(
                !msg.contains(shed),
                "{shed} should be shed, not printed: {msg}"
            );
        }
        assert!(msg.contains("Per-turn tokens: /tokens"), "{msg}");
        // The footer-item *keys* were a full-width row of internal config
        // names; `/statusline` is the surface that owns them.
        assert!(!msg.contains("reasoning_replay"), "{msg}");
        assert!(!msg.contains("git_branch"), "{msg}");
        assert!(msg.contains("Footer items: /statusline"), "{msg}");

        let row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Session tokens:"))
            .expect("session tokens row");
        assert!(row.contains("900 in"), "{row}");
        assert!(row.contains("120 out"), "{row}");
        assert!(row.contains("1020 total"), "{row}");
        assert!(row.contains("cache 700 hit / 200 miss"), "{row}");
    }

    /// Provider, model and effort are one fact — which route this turn goes
    /// to — and the header rail already renders them as one dotted lockup.
    #[test]
    fn status_report_states_the_route_the_way_the_header_does() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        let msg = status(&mut app).message.expect("status message");
        assert!(!msg.contains("Provider:"), "{msg}");
        assert!(!msg.contains("Model:"), "{msg}");
        let row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Route:"))
            .expect("route row");
        assert!(row.contains(" · "), "route must read as a lockup: {row}");
        assert!(row.contains("reasoning"), "{row}");
    }

    /// #5134: the number alone sends users to the issue tracker. `/status` has
    /// to name the provenance and the key that changes it, and it must name the
    /// table the user is actually on — not a generic placeholder. The two are
    /// separate facts, so the key gets its own aligned row instead of a
    /// parenthesis that pushed the provenance off the end of an 80-column line.
    #[test]
    fn status_report_names_context_window_source_and_override_key() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.api_provider = ApiProvider::Moonshot;

        let msg = status(&mut app).message.expect("status message");

        let source_row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Window source:"))
            .expect("window source row");
        assert!(
            !source_row.contains("context_window"),
            "the provenance row states the provenance only: {source_row}"
        );
        // A labelled row, not an indented continuation: the transcript cell
        // strips leading whitespace, so an aligned continuation line rendered
        // flush against the label column and read as a field of its own with
        // the label missing.
        let override_row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Window override:"))
            .expect("window override row");
        assert!(
            override_row.contains("[providers.moonshot] context_window in config.toml"),
            "{override_row}"
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
        assert!(!msg.contains("Window override:"), "{msg}");
    }

    #[test]
    fn status_report_keeps_exact_named_custom_provider() {
        let tmpdir = TempDir::new().expect("temp dir");
        let mut app = create_test_app(tmpdir.path().to_path_buf());
        app.set_provider_identity(ApiProvider::Custom, "lm-studio");

        let msg = status(&mut app).message.expect("status message");

        let route_row = msg
            .lines()
            .find(|line| line.trim_start().starts_with("Route:"))
            .expect("route row");
        assert!(route_row.contains("lm-studio"), "{route_row}");
        assert!(!route_row.contains("custom"), "{route_row}");
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
            // workspace-write no longer implies egress; /status must say so.
            assert!(agent.contains("sandbox workspace-write, network off"));
        }

        app.approval_mode = crate::tui::approval::ApprovalMode::Bypass;
        let full_access = format_status(&app);
        assert!(full_access.contains("sandbox disabled, network unrestricted"));

        app.configured_sandbox_mode = Some("workspace-write".to_string());
        let clamped = format_status(&app);
        if unenforced {
            assert!(clamped.contains("workspace-write requested, not enforced"));
        } else {
            // Clamping full access down to workspace-write lands on the same
            // restricted posture an ordinary Agent turn gets.
            assert!(clamped.contains("sandbox workspace-write, network off"));
        }

        // The explicit opt-in is the only thing that flips the reported label.
        app.configured_sandbox_network = Some(true);
        let networked = format_status(&app);
        if unenforced {
            assert!(networked.contains("workspace-write requested, not enforced"));
        } else {
            assert!(networked.contains("sandbox workspace-write, network on"));
        }
        app.configured_sandbox_network = None;

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
            role: Role::User,
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
        assert_eq!(project_docs(tmpdir.path()), "no project docs");
    }
}
