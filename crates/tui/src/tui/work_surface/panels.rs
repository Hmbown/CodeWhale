//! Non-Tasks rail panels, ported from the legacy classic-shell sidebar
//! during the 0.9.4 rail unification (spec step 2). Agents, Context, and
//! Pinned render as titled line lists inside the one work-surface rail, in
//! whatever placement the user picked; panel selection is orthogonal to
//! placement. The Tasks panel is *not* here — it renders through the
//! row/hitbox machinery in `render.rs`.
//!
//! The line builders themselves still live in `tui::sidebar` (they are
//! `pub(crate)` there) while the sidebar module is wound down; the rail is
//! their only production caller now.

use ratatui::text::Line;

use crate::tui::app::App;
use crate::tui::sidebar::{self, SidebarSubagentSummary};
use crate::tui::subagent_routing::active_fanout_counts;

use super::model::RailPanel;

/// Display lines for a non-Tasks rail panel, or `None` for Tasks (which the
/// caller renders through the row machinery instead).
pub(crate) fn panel_lines(
    app: &mut App,
    panel: RailPanel,
    content_width: usize,
    max_rows: usize,
) -> Option<Vec<Line<'static>>> {
    let content_width = content_width.max(1);
    let max_rows = max_rows.max(1);
    match panel {
        RailPanel::Tasks => None,
        RailPanel::Agents => Some(agents_panel_lines(app, content_width, max_rows)),
        RailPanel::Context => Some(sidebar::context_panel_lines(app, content_width)),
        RailPanel::Pinned => Some(pinned_panel_lines(app, content_width, max_rows)),
    }
}

/// Agents panel: cached sub-agents plus progress-only and fanout signals.
/// The summary projection is lifted from the legacy `render_sidebar_subagents`
/// so the panel keeps its exact content in the rail.
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
    sidebar::subagent_panel_lines(
        &summary,
        &rows,
        app.ui_locale,
        content_width,
        max_rows,
        &app.ui_theme,
    )
}

/// Pinned panel: the durable work summary (goal + checklist) the legacy
/// sidebar showed in Pinned focus.
fn pinned_panel_lines(app: &mut App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let summary = sidebar::sidebar_work_summary(app);
    sidebar::work_panel_lines(
        &summary,
        content_width,
        max_rows,
        app.ui_theme.mode,
        &app.ui_theme,
    )
}
