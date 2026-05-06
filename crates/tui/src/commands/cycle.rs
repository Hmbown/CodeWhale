//! Cycle commands: `/cycles` (list past cycle boundaries) and
//! `/cycle <n>` (show one cycle's briefing in detail).

use std::fmt::Write;

use crate::tui::app::App;

use super::CommandResult;

/// `/cycles` — list past cycle handoffs in compact form.
pub fn list_cycles(app: &App) -> CommandResult {
    if app.cycle_briefings.is_empty() {
        let msg = format!(
            "No cycle boundaries have fired yet (current cycle: 1, threshold: {} tokens for {}).",
            app.cycle.threshold_for(&app.model),
            app.model
        );
        return CommandResult::message(msg);
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "Cycle handoffs in this session ({} total). Active cycle: {}.",
        app.cycle_briefings.len(),
        app.cycle_count.saturating_add(1),
    );
    out.push('\n');
    for brief in &app.cycle_briefings {
        let preview = first_line(&brief.briefing_text, 80);
        let _ = writeln!(
            out,
            "  cycle {n}  @ {ts}  briefing: {tokens} tokens  ─ {preview}",
            n = brief.cycle,
            ts = brief.timestamp.to_rfc3339(),
            tokens = brief.token_estimate,
            preview = preview,
        );
    }
    out.push('\n');
    out.push_str("Use `/cycle <n>` to show the full briefing for a specific cycle.\n");
    CommandResult::message(out)
}

/// `/cycle <n>` — print the full briefing for cycle `n`.
pub fn show_cycle(app: &App, arg: Option<&str>) -> CommandResult {
    let Some(raw) = arg.map(str::trim) else {
        return CommandResult::error(
            "Usage: /cycle <n>  — n is the cycle number from /cycles".to_string(),
            app.ui_locale,
        );
    };
    if raw.is_empty() {
        return CommandResult::error("Usage: /cycle <n>".to_string(), app.ui_locale);
    }
    let Ok(n) = raw.parse::<u32>() else {
        return CommandResult::error(
            format!("Cycle number must be a positive integer (got '{raw}')."),
            app.ui_locale,
        );
    };

    let Some(brief) = app.cycle_briefings.iter().find(|b| b.cycle == n) else {
        let known: Vec<String> = app
            .cycle_briefings
            .iter()
            .map(|b| b.cycle.to_string())
            .collect();
        let known_str = if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        };
        return CommandResult::error(
            format!("Cycle {n} not found in this session. Known cycles: {known_str}."),
            app.ui_locale,
        );
    };

    let mut out = String::new();
    let _ = writeln!(
        out,
        "── Cycle {n}  ({ts})  briefing: {tokens} tokens ──",
        n = brief.cycle,
        ts = brief.timestamp.to_rfc3339(),
        tokens = brief.token_estimate,
    );
    out.push('\n');
    out.push_str(brief.briefing_text.trim());
    out.push('\n');
    CommandResult::message(out)
}

/// `/recall <query>` — user-initiated BM25 search of cycle archives.
///
/// Synchronous wrapper around `tools::recall_archive::RecallArchiveTool` so
/// users can probe the archive without invoking the model. Output is the
/// same JSON payload the agent would see; the assistant pretty-prints
/// short results and dumps long ones inline.
pub fn recall_archive(app: &App, arg: Option<&str>) -> CommandResult {
    use crate::tools::recall_archive::RecallArchiveTool;
    use crate::tools::spec::{ToolContext, ToolSpec};

    let Some(raw) = arg.map(str::trim) else {
        return CommandResult::error("Usage: /recall <query>".to_string(), app.ui_locale);
    };
    if raw.is_empty() {
        return CommandResult::error("Usage: /recall <query>".to_string(), app.ui_locale);
    }

    let session_id = app
        .current_session_id
        .clone()
        .unwrap_or_else(|| "workspace".to_string());

    let context = ToolContext::new(app.workspace.clone()).with_state_namespace(session_id);
    let tool = RecallArchiveTool;
    let input = serde_json::json!({"query": raw});

    let result = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(tool.execute(input, &context))
    });

    match result {
        Ok(res) => CommandResult::message(res.content),
        Err(err) => CommandResult::error(format!("recall_archive failed: {err}"), app.ui_locale),
    }
}

/// Truncate `text` to its first non-empty line, capped at `max_chars`.
fn first_line(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max_chars {
        line.to_string()
    } else {
        let prefix: String = line.chars().take(max_chars).collect();
        format!("{prefix}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cycle_manager::CycleBriefing;
    use crate::tui::app::{App, TuiOptions};
    use chrono::Utc;
    use std::path::PathBuf;

    fn test_options() -> TuiOptions {
        TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        }
    }

    #[test]
    fn list_cycles_reports_no_boundaries_yet() {
        let app = App::new(test_options(), &crate::config::Config::default());
        let res = list_cycles(&app);
        assert!(res.message.is_some());
        assert!(
            res.message
                .as_deref()
                .unwrap()
                .contains("No cycle boundaries")
        );
    }

    #[test]
    fn show_cycle_rejects_nonexistent_cycle() {
        let app = App::new(test_options(), &crate::config::Config::default());
        let res = show_cycle(&app, Some("3"));
        let msg = res.message.expect("error message");
        assert!(msg.contains("Cycle 3 not found"), "got: {msg}");
    }

    #[test]
    fn list_and_show_cycles_render_briefings() {
        let mut app = App::new(test_options(), &crate::config::Config::default());
        app.cycle_briefings.push(CycleBriefing {
            cycle: 1,
            timestamp: Utc::now(),
            briefing_text: "Decision: chose A; constraint: no async.".to_string(),
            token_estimate: 12,
        });
        app.cycle_count = 1;

        let listed = list_cycles(&app).message.expect("list message");
        assert!(listed.contains("cycle 1"));
        assert!(listed.contains("12 tokens"));

        let shown = show_cycle(&app, Some("1")).message.expect("show message");
        assert!(shown.contains("Decision: chose A"));
    }

    #[test]
    fn show_cycle_validates_argument() {
        let app = App::new(test_options(), &crate::config::Config::default());
        let res = show_cycle(&app, None);
        let msg = res.message.expect("error message");
        assert!(msg.contains("Usage: /cycle"));

        let res = show_cycle(&app, Some("not-a-number"));
        let msg = res.message.expect("error message");
        assert!(msg.contains("must be a positive integer"));
    }
}
