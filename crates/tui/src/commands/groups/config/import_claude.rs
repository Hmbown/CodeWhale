//! `/import-claude` — explicit, reviewable Claude Code migration (#5557).

use crate::commands::CommandResult;
use crate::import_claude::{self, McpCandidateLine};
use crate::tui::app::App;

pub(super) fn import_claude_command(app: &mut App, _arg: Option<&str>) -> CommandResult {
    let home = crate::config::effective_home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let (sources, claude, settings) = import_claude::read_sources(&home);

    // MCP candidates come from the same discovery the `/mcp import` consent
    // flow uses, filtered to the Claude sources: provenance stays single-sourced.
    let markets = Vec::new();
    let mcp_candidates =
        crate::mcp::external_import::discover_external_sources(&home, &app.workspace, &markets)
            .into_iter()
            .filter(|candidate| {
                matches!(
                    candidate.source_kind,
                    crate::mcp::external_import::ExternalMcpSourceKind::ClaudeJson
                )
            })
            .map(|candidate| McpCandidateLine {
                summary: candidate.summary,
                hard_blocked: candidate.hard_blocked,
                name: candidate.name,
            })
            .collect::<Vec<_>>();

    let plan = import_claude::build_plan(sources, claude, settings, &home, mcp_candidates);
    if plan.is_empty() {
        return CommandResult {
            message: Some(
                "No Claude configuration found to import (looked for ~/.claude.json and \
                 ~/.claude/settings.json)."
                    .to_string(),
            ),
            action: None,
            is_error: false,
        };
    }

    // The plan is shown before anything is written; the only writes are the
    // report and an *unapplied* bundle file. Applying always goes through a
    // separate consent path (`/mcp import <name> --approve`, `config import`).
    let imports_dir = codewhale_config::codewhale_home()
        .map(|home| home.join("imports"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".codewhale/imports"));
    let report_path = imports_dir.join("claude-import-report.md");
    let bundle_path = imports_dir.join("claude-portable-bundle.json");
    let wrote_report = codewhale_config::persistence::atomic_write(
        &report_path,
        import_claude::report_markdown(&plan).as_bytes(),
    )
    .is_ok();
    let wrote_bundle = if plan.env_safe.is_empty() {
        false
    } else {
        codewhale_config::persistence::atomic_write(
            &bundle_path,
            import_claude::portable_bundle_json(&plan).as_bytes(),
        )
        .is_ok()
    };

    let mut message = import_claude::render_plan(&plan);
    if wrote_report {
        message.push_str(&format!(
            "\nFull report: {}",
            crate::utils::display_path(&report_path)
        ));
    } else {
        message.push_str("\n(the report could not be written; the plan above is complete)");
    }
    if wrote_bundle {
        message.push_str(&format!(
            "\nPortable bundle (review, then `codewhale config import {}`): apply it there for the consent/rollback path.",
            crate::utils::display_path(&bundle_path)
        ));
    }
    CommandResult {
        message: Some(message),
        action: None,
        is_error: false,
    }
}
