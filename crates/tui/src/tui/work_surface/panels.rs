//! Line-list rail panels, ported from the legacy classic-shell sidebar
//! during the 0.9.4 rail unification (spec step 2). Only **Context** still
//! renders this way: its lines are session facts, not work rows, so there
//! is nothing to click. Tasks, Agents, and Pinned all render through the
//! row/hitbox machinery in `render.rs` — a work row is a selectable,
//! clickable object in every panel, and a panel is just a subset of the one
//! work list.
//!
//! On Top placement the strip auto-fits its content the way Tasks always
//! did (and the way GrokBuild's tasks pane does): a two-agent fan-out is
//! two rows, not a fixed four-row band with a chrome title. Auto-fit
//! governs HEIGHT only, never membership — a settled to-do or finished
//! sub-agent still occupies a row (quiet completion, not eviction). The
//! only Top title is an active **goal** (not panel names like "Pinned").
//!
//! The line builders themselves still live in `tui::sidebar` (they are
//! `pub(crate)` there) while the sidebar module is wound down. The Agents
//! and Pinned arms below are retained for that wind-down but are no longer
//! reachable from the rail, which routes those panels through rows.

use ratatui::text::Line;

use crate::tui::app::App;
use crate::tui::sidebar::{self, SidebarSubagentSummary, WorkPanelOpts};
use crate::tui::subagent_routing::active_fanout_counts;

use super::model::RailPanel;

/// Cap used when measuring natural content height so a pathological
/// checklist cannot allocate unbounded lines during layout. The strip's
/// real cap (`top_cap`) still clamps the visible window.
const NATURAL_HEIGHT_PROBE: usize = 64;

/// Display lines for a non-Tasks rail panel, or `None` for Tasks (which the
/// caller renders through the row machinery instead).
///
/// `omit_goal_objective` is set on Top when the goal is already the strip
/// title, so Pinned does not repeat `Goal: …` in the body.
pub(crate) fn panel_lines(
    app: &mut App,
    panel: RailPanel,
    content_width: usize,
    max_rows: usize,
    omit_goal_objective: bool,
) -> Option<Vec<Line<'static>>> {
    let content_width = content_width.max(1);
    let max_rows = max_rows.max(1);
    match panel {
        RailPanel::Tasks => None,
        RailPanel::Agents => Some(agents_panel_lines(app, content_width, max_rows)),
        RailPanel::Context => Some(sidebar::context_panel_lines(app, content_width)),
        RailPanel::Pinned => Some(pinned_panel_lines(
            app,
            content_width,
            max_rows,
            omit_goal_objective,
        )),
    }
}

/// Whether a non-Tasks panel has anything worth spending a top-strip row on.
/// Empty projections collapse to zero the way Tasks does — an empty panel is
/// not a panel. Context always has session facts, so it always has content.
pub(crate) fn panel_has_useful_content(app: &mut App, panel: RailPanel) -> bool {
    match panel {
        RailPanel::Tasks => true,
        RailPanel::Pinned => sidebar::sidebar_work_summary(app).has_useful_content(),
        RailPanel::Agents => agents_have_useful_content(app),
        RailPanel::Context => true,
    }
}

/// Natural content row count for height auto-fit. Does not include the
/// divider row or the optional Top goal title that `height()` adds.
pub(crate) fn panel_content_row_count(
    app: &mut App,
    panel: RailPanel,
    content_width: usize,
    omit_goal_objective: bool,
) -> usize {
    panel_lines(
        app,
        panel,
        content_width,
        NATURAL_HEIGHT_PROBE,
        omit_goal_objective,
    )
    .map(|lines| lines.len())
    .unwrap_or(0)
}

fn agents_have_useful_content(app: &App) -> bool {
    if active_watchers(app).next().is_some() {
        return true;
    }
    if !app.subagent_cache.is_empty() {
        return true;
    }
    if !app.agent_progress.is_empty() {
        return true;
    }
    if active_fanout_counts(app).is_some_and(|(_, total)| total > 0) {
        return true;
    }
    sidebar::foreground_rlm_running(app)
}

/// Agents panel: cached sub-agents plus progress-only and fanout signals.
/// The summary projection is lifted from the legacy `render_sidebar_subagents`
/// so the panel keeps its exact content in the rail.
fn active_watchers(app: &App) -> impl Iterator<Item = crate::automation_manager::AutomationRecord> {
    let listed = app
        .runtime_services
        .automations
        .as_ref()
        .and_then(|manager| manager.try_lock().ok())
        .and_then(|guard| guard.list_automations().ok())
        .unwrap_or_default();
    listed.into_iter().filter(|record| {
        record.status == crate::automation_manager::AutomationStatus::Active
            && record.delivery_mode() == crate::automation_manager::AutomationDeliveryMode::Watcher
    })
}

fn watcher_lines(app: &App, content_width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let watchers: Vec<_> = active_watchers(app).collect();
    if watchers.is_empty() {
        return lines;
    }
    lines.push(Line::from(
        crate::localization::tr(
            app.ui_locale,
            crate::localization::MessageId::AutomationWatchersHeading,
        )
        .into_owned(),
    ));
    for record in watchers {
        let next = record
            .next_run_at
            .map(|when| {
                let remaining = when.signed_duration_since(chrono::Utc::now());
                if remaining.num_seconds() <= 0 {
                    "due".to_string()
                } else if remaining.num_hours() >= 1 {
                    format!("next in {}h", remaining.num_hours())
                } else {
                    format!("next in {}m", remaining.num_minutes().max(1))
                }
            })
            .unwrap_or_else(|| "scheduled".to_string());
        let name = if record.name.chars().count() > content_width.saturating_sub(16) {
            record
                .name
                .chars()
                .take(content_width.saturating_sub(19))
                .collect::<String>()
                + "..."
        } else {
            record.name.clone()
        };
        lines.push(Line::from(format!("Loop {next} · {name}")));
    }
    lines
}

fn agents_panel_lines(app: &App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let cached_ids: std::collections::HashSet<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    let progress_only_count = app
        .agent_progress
        .keys()
        .filter(|id| !cached_ids.contains(id.as_str()))
        .count();
    let cached_running = app
        .subagent_cache
        .iter()
        .filter(|agent| sidebar::cached_agent_activity_is_live(app, agent))
        .count();
    let role_counts: std::collections::BTreeMap<String, usize> =
        app.subagent_cache
            .iter()
            .fold(std::collections::BTreeMap::new(), |mut acc, agent| {
                *acc.entry(agent.agent_type.as_str().to_string())
                    .or_insert(0) += 1;
                acc
            });
    let (fanout_running, fanout_total) = active_fanout_counts(app)
        .map(|(running, total)| (running, Some(total)))
        .unwrap_or((0, None));
    let summary = SidebarSubagentSummary {
        cached_total: app.subagent_cache.len(),
        cached_running,
        progress_only_count,
        fanout_total,
        fanout_running,
        foreground_rlm_running: sidebar::foreground_rlm_running(app),
        role_counts,
    };
    let rows = sidebar::sidebar_agent_rows(app);
    let mut lines = watcher_lines(app, content_width);
    let remaining = max_rows.saturating_sub(lines.len()).max(1);
    lines.extend(sidebar::subagent_panel_lines(
        &summary,
        &rows,
        app.ui_locale,
        content_width,
        remaining,
        &app.ui_theme,
    ));
    lines
}

/// Pinned panel: the durable work summary (goal + checklist) the legacy
/// sidebar showed in Pinned focus.
fn pinned_panel_lines(
    app: &mut App,
    content_width: usize,
    max_rows: usize,
    omit_goal_objective: bool,
) -> Vec<Line<'static>> {
    let summary = sidebar::sidebar_work_summary(app);
    sidebar::work_panel_lines_with_opts(
        &summary,
        content_width,
        max_rows,
        app.ui_theme.mode,
        &app.ui_theme,
        WorkPanelOpts {
            omit_goal_objective,
        },
    )
}
