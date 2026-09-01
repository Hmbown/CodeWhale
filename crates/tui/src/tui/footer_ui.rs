use crate::tui::app::App;
use crate::tui::history::summarize_tool_output;
use crate::tui::subagent_routing::{active_fanout_counts, running_agent_count};
use crate::tui::ui_text::truncate_line_to_width;

/// Seconds the current turn has gone without observable stream activity.
pub(crate) fn provider_wait_idle_secs(app: &App) -> u64 {
    app.turn_last_activity_at
        .or(app.turn_started_at)
        .map(|at| at.elapsed().as_secs())
        .unwrap_or(0)
}

/// Threshold after which a provider wait with a planned fanout is logged as
/// a structured incident (once per turn).
const PROVIDER_WAIT_INCIDENT_SECS: u64 = 120;

/// Log a compact structured incident when the parent turn has spent a long
/// time in provider wait while a sub-agent fanout plan is present (#3095).
pub(crate) fn maybe_log_provider_wait_incident(app: &mut App) {
    if app.provider_wait_incident_logged || !app.is_loading {
        return;
    }
    let elapsed = match app.turn_started_at {
        Some(at) => at.elapsed().as_secs(),
        None => return,
    };
    if elapsed < PROVIDER_WAIT_INCIDENT_SECS {
        return;
    }
    let fanout = active_fanout_counts(app);
    let pending_dispatch = app.pending_subagent_dispatch.is_some();
    if fanout.is_none() && !pending_dispatch {
        return;
    }
    let (fanout_running, fanout_total) = fanout.unwrap_or((0, 0));
    app.provider_wait_incident_logged = true;
    crate::logging::warn(format!(
        "provider-wait incident: provider={} model={} elapsed_secs={elapsed} \
         idle_secs={} stream_idle_budget_secs={} max_subagents={} \
         fanout_running={fanout_running} fanout_total={fanout_total} \
         running_agents={} pending_dispatch={pending_dispatch}",
        app.provider_identity_for_persistence(),
        app.model,
        provider_wait_idle_secs(app),
        app.stream_chunk_timeout_secs,
        app.max_subagents,
        running_agent_count(app),
    ));
}

pub(crate) fn is_noisy_subagent_progress(status: &str) -> bool {
    let status = status.trim().to_ascii_lowercase();
    status.contains("requesting model response")
}

pub(crate) fn subagent_objective_summary(app: &App, id: &str) -> Option<String> {
    app.subagent_cache
        .iter()
        .find(|agent| agent.agent_id == id)
        .map(|agent| summarize_tool_output(&agent.assignment.objective))
        .filter(|summary| !summary.is_empty())
}

pub(crate) fn friendly_subagent_progress(app: &App, id: &str, status: &str) -> String {
    if !is_noisy_subagent_progress(status) {
        return summarize_tool_output(status);
    }

    if let Some(summary) = subagent_objective_summary(app, id) {
        return format!("working on {summary}");
    }
    if let Some(existing) = app.agent_progress.get(id)
        && !is_noisy_subagent_progress(existing)
        && existing != "working"
        && existing != "in the current"
    {
        return existing.clone();
    }
    "in the current".to_string()
}

pub(crate) fn one_line_summary(text: &str, max_width: usize) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    truncate_line_to_width(
        &cleaned.split_whitespace().collect::<Vec<_>>().join(" "),
        max_width,
    )
}

/// Objective + paused flag for the live goal, or `None` when no goal should
/// render (unset, or terminal Hunted/Escaped). Shared by the classic footer
/// chip and the ocean topbar chip so every shell surfaces the same state
/// (#39: the ocean shell has no sidebar, so without a topbar chip a goal set
/// via `create_goal` was invisible there).
pub(crate) fn active_goal_chip_state(app: &App) -> Option<(String, bool)> {
    let (objective, paused) = match (&app.goal.objective, &app.paused_goal_objective) {
        (Some(objective), _) => {
            if matches!(
                app.goal.status,
                crate::tools::goal::GoalStatus::Complete | crate::tools::goal::GoalStatus::Blocked
            ) {
                return None;
            }
            (
                objective.clone(),
                app.goal.status == crate::tools::goal::GoalStatus::Paused,
            )
        }
        (None, Some(objective)) => (objective.clone(), true),
        (None, None) => return None,
    };
    if objective.trim().is_empty() {
        return None;
    }
    Some((objective, paused))
}

pub(crate) fn format_token_count_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
pub(crate) fn format_context_budget(used: i64, max: u32) -> String {
    let max_u64 = u64::from(max);
    let max_i64 = i64::from(max);

    if used > max_i64 {
        return format!(
            ">{}/{}",
            format_token_count_compact(max_u64),
            format_token_count_compact(max_u64)
        );
    }

    let used_u64 = u64::try_from(used.max(0)).unwrap_or(0);
    format!(
        "{}/{}",
        format_token_count_compact(used_u64),
        format_token_count_compact(max_u64)
    )
}

#[cfg(test)]
mod tests {
    use super::one_line_summary;

    #[test]
    fn one_line_summary_strips_ansi_before_collapsing_text() {
        let summary = one_line_summary("read \x1b[38;2;6;174;242mfile.rs\x1b[0m", 80);
        assert_eq!(summary, "read file.rs");
        assert!(!summary.contains("38;2"));
    }
}
