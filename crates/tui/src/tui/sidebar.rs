//! Sidebar rendering — Pinned / Activity / Agents / Context panels.
//!
//! Extracted from `tui/ui.rs` (P1.2). The sidebar appears to the right of
//! the chat transcript when the available width allows it. Each section
//! reads from `App` snapshots; mutation lives in the main app loop.

use std::time::Instant;

use crate::localization::Locale;
use crate::tools::goal::GoalStatus;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::palette;
use crate::tools::subagent::{AgentWorkerStatus, SubAgentStatus, localized_whale_display_names};
use crate::tools::todo::TodoStatus;

use super::app::{AgentCurrentActivity, AgentCurrentActivityStatus, App, SidebarRowAction};
use super::history::{HistoryCell, ToolCell, ToolStatus, summarize_tool_output};
use super::ui_text::truncate_line_to_width;

/// Tolerance for floating-point cost comparison in the sidebar breakdown.
/// Must be large enough that accumulated f64 error across hundreds of turns
/// does not prematurely hide the session+agents breakdown.
const COST_EQ_TOLERANCE: f64 = 1e-6;
const TASK_STOP_TARGET_LABEL: &str = "[x]";
const TASK_STOP_TARGET_SUFFIX: &str = " [x]";
#[derive(Debug, Clone)]
struct SidebarWorkChecklistItem {
    id: u32,
    content: String,
    status: TodoStatus,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarWorkSummary {
    goal_objective: Option<String>,
    goal_token_budget: Option<u32>,
    goal_completed: bool,
    goal_started_at: Option<Instant>,
    /// When the goal went terminal. While `Some`, the elapsed line freezes at
    /// `goal_finished_at - goal_started_at` instead of ticking every frame.
    goal_finished_at: Option<Instant>,
    tokens_used: u32,
    checklist_completion_pct: u8,
    checklist_items: Vec<SidebarWorkChecklistItem>,
    state_updating: bool,
    pause_indicator: Option<String>,
    workflow_paused: bool,
}

impl SidebarWorkSummary {
    pub(crate) fn has_useful_content(&self) -> bool {
        self.goal_objective
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || !self.checklist_items.is_empty()
            || self.state_updating
    }
}

/// Objective of the active goal, if any. Paused goals keep showing their
/// quarry; the work summary uses this so a completed goal can still render
/// with its DONE state.
pub(crate) fn live_goal_objective(app: &App) -> Option<String> {
    if app.paused || app.paused_goal_objective.is_some() {
        app.goal
            .objective
            .clone()
            .or_else(|| app.paused_goal_objective.clone())
    } else {
        app.goal.objective.clone()
    }
}

pub(crate) fn sidebar_work_summary(app: &mut App) -> SidebarWorkSummary {
    fn live_pause_indicator(app: &App) -> Option<String> {
        if app.paused && app.is_loading {
            Some("(Pausing)".to_string())
        } else if app.paused || app.paused_goal_objective.is_some() {
            Some("(Paused)".to_string())
        } else if app.goal.status == GoalStatus::Paused {
            Some(match app.goal.pause_reason {
                Some(reason) => format!("(Paused: {})", reason.label()),
                None => "(Paused)".to_string(),
            })
        } else {
            None
        }
    }

    fn apply_live_goal_state(summary: &mut SidebarWorkSummary, app: &App) {
        summary.goal_objective = live_goal_objective(app);
        summary.goal_token_budget = app.goal.token_budget;
        summary.goal_completed = app.goal.status == GoalStatus::Complete;
        summary.goal_started_at = app.goal.started_at;
        summary.goal_finished_at = app.goal.finished_at;
        summary.tokens_used = app.session.total_conversation_tokens;
        summary.pause_indicator = live_pause_indicator(app);
        summary.workflow_paused = app.paused
            || app.paused_goal_objective.is_some()
            || app.goal.status == GoalStatus::Paused;
    }

    let fresh = (|| {
        let todos = app.todos.try_lock().ok()?;
        let snapshot = todos.snapshot();
        let checklist_completion_pct = snapshot.completion_pct;
        let checklist_items = snapshot
            .items
            .into_iter()
            .map(|item| SidebarWorkChecklistItem {
                id: item.id,
                content: item.content,
                status: item.status,
            })
            .collect();

        let mut summary = SidebarWorkSummary {
            goal_objective: live_goal_objective(app),
            goal_token_budget: app.goal.token_budget,
            goal_completed: app.goal.status == GoalStatus::Complete,
            goal_started_at: app.goal.started_at,
            goal_finished_at: app.goal.finished_at,
            tokens_used: app.session.total_conversation_tokens,
            checklist_completion_pct,
            checklist_items,
            // Strategy/plan remains compatibility state for saved sessions,
            // but it is not a second user-facing progress surface.
            state_updating: false,
            pause_indicator: live_pause_indicator(app),
            workflow_paused: app.paused
                || app.paused_goal_objective.is_some()
                || app.goal.status == GoalStatus::Paused,
        };
        apply_live_goal_state(&mut summary, app);
        Some(summary)
    })();

    if let Some(summary) = fresh {
        app.cached_work_summary = Some(summary.clone());
        return summary;
    }

    if let Some(cached) = app.cached_work_summary.as_ref() {
        let mut summary = cached.clone();
        apply_live_goal_state(&mut summary, app);
        return summary;
    }

    let mut summary = SidebarWorkSummary {
        state_updating: true,
        ..SidebarWorkSummary::default()
    };
    apply_live_goal_state(&mut summary, app);
    summary
}

/// Default-options shorthand for [`work_panel_lines_with_opts`].
///
/// Production callers all pass real [`WorkPanelOpts`] since the goal title
/// moved to the strip, so this only survives to keep the tests readable.
#[cfg(test)]
pub(crate) fn work_panel_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    palette_mode: palette::PaletteMode,
    ui_theme: &palette::UiTheme,
) -> Vec<Line<'static>> {
    work_panel_lines_with_opts(
        summary,
        content_width,
        max_rows,
        palette_mode,
        ui_theme,
        WorkPanelOpts::default(),
    )
}

/// Options for the Pinned work panel body.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct WorkPanelOpts {
    /// When true, skip the primary `Goal: …` objective line. Used on Top
    /// placement where that line is already the strip title — repeating it
    /// in the body wastes a scarce row.
    pub omit_goal_objective: bool,
}

pub(crate) fn work_panel_lines_with_opts(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    palette_mode: palette::PaletteMode,
    ui_theme: &palette::UiTheme,
    opts: WorkPanelOpts,
) -> Vec<Line<'static>> {
    let _ = palette_mode;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    push_work_goal_lines(
        summary,
        content_width,
        max_rows,
        &mut lines,
        ui_theme,
        opts.omit_goal_objective,
    );

    if summary.state_updating && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            "To-do updating...",
            Style::default().fg(ui_theme.text_muted),
        )));
    }

    push_work_checklist_lines(summary, content_width, max_rows, &mut lines, ui_theme);

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            work_panel_empty_hint(content_width),
            Style::default().fg(ui_theme.text_muted).italic(),
        )));
    }

    lines
}

/// Humanized elapsed time for a goal. Once the goal is terminal (`finished`
/// is `Some`), the elapsed is frozen at `finished - started` so a completed or
/// escaped goal stops ticking in the sidebar; otherwise it grows live.
fn goal_elapsed_for_summary(started: Instant, finished: Option<Instant>) -> String {
    let elapsed = match finished {
        Some(end) => end.saturating_duration_since(started),
        None => started.elapsed(),
    };
    crate::elapsed::format_elapsed_secs(elapsed.as_secs())
}

fn push_work_goal_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
    theme: &palette::UiTheme,
    omit_objective: bool,
) {
    let Some(objective) = summary.goal_objective.as_deref() else {
        return;
    };
    if objective.trim().is_empty() || lines.len() >= max_rows {
        return;
    }

    if !omit_objective {
        let icon = if summary.goal_completed {
            crate::tui::glyphs::DONE
        } else if summary.workflow_paused {
            crate::tui::glyphs::PAUSED
        } else {
            crate::tui::glyphs::ATTENTION
        };
        let status_style = if summary.goal_completed {
            Style::default()
                .fg(theme.success)
                .add_modifier(ratatui::style::Modifier::BOLD)
        } else {
            Style::default()
                .fg(theme.warning)
                .add_modifier(ratatui::style::Modifier::BOLD)
        };
        // Show the full goal objective — this is goal mode's primary status
        // surface. Prefix with "Goal:" so the compact row is clearly labelled
        // as a goal-mode objective, not a generic session title.
        let label = if let Some(indicator) = summary.pause_indicator.as_deref() {
            format!("Goal: {objective} {indicator}")
        } else {
            format!("Goal: {objective}")
        };

        lines.push(Line::from(Span::styled(
            format!(
                "{} {}",
                icon,
                truncate_line_to_width(&label, content_width.saturating_sub(2).max(1))
            ),
            status_style,
        )));
    }

    // Elapsed time
    if let Some(started) = summary.goal_started_at
        && lines.len() < max_rows
    {
        let elapsed = goal_elapsed_for_summary(started, summary.goal_finished_at);
        let elapsed_str = if summary.goal_completed {
            format!("completed in {elapsed}")
        } else {
            format!("elapsed: {elapsed}")
        };
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&elapsed_str, content_width),
            Style::default().fg(theme.text_muted),
        )));
    }

    if let Some(budget) = summary.goal_token_budget
        && lines.len() < max_rows
    {
        let pct = if budget > 0 {
            ((summary.tokens_used as f64 / budget as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        let bar_width = content_width.min(20);
        let filled = ((pct / 100.0) * bar_width as f64) as usize;
        let bar = format!(
            "[{}{}] {:.0}%",
            "█".repeat(filled),
            "░".repeat(bar_width.saturating_sub(filled)),
            pct
        );
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(
                &format!("tokens: {}/{} {}", summary.tokens_used, budget, bar),
                content_width,
            ),
            Style::default().fg(theme.text_muted),
        )));
    }
}

fn push_work_checklist_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
    theme: &palette::UiTheme,
) {
    if summary.checklist_items.is_empty() || lines.len() >= max_rows {
        return;
    }

    let total = summary.checklist_items.len();
    let settled = summary
        .checklist_items
        .iter()
        .filter(|item| item.status.is_settled())
        .count();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}%", summary.checklist_completion_pct),
            Style::default().fg(theme.success).bold(),
        ),
        Span::styled(
            format!(" settled ({settled}/{total})"),
            Style::default().fg(theme.text_muted),
        ),
    ]));

    let available_item_rows = max_rows
        .saturating_sub(lines.len())
        .min(summary.checklist_items.len());
    let max_items =
        if summary.checklist_items.len() > available_item_rows && available_item_rows > 1 {
            available_item_rows - 1
        } else {
            available_item_rows
        };
    let start = checklist_window_start(&summary.checklist_items, max_items);
    let end = start
        .saturating_add(max_items)
        .min(summary.checklist_items.len());
    for item in summary.checklist_items[start..end].iter() {
        let (prefix, style) = match item.status {
            TodoStatus::Pending => ("[ ]", Style::default().fg(theme.text_muted)),
            TodoStatus::InProgress => (
                "[~]",
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            ),
            TodoStatus::Completed => ("[✓]", Style::default().fg(theme.success)),
            TodoStatus::Cancelled => (
                "[-]",
                Style::default()
                    .fg(theme.error_fg)
                    .add_modifier(ratatui::style::Modifier::CROSSED_OUT),
            ),
        };
        let text = format!("{prefix} #{} {}", item.id, item.content);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&text, content_width),
            style,
        )));
    }

    let earlier = start;
    let later = summary.checklist_items.len().saturating_sub(end);
    let remaining = earlier.saturating_add(later);
    if remaining > 0 && lines.len() < max_rows {
        let label = match (earlier, later) {
            (0, later) => format!("+{later} more To-do items"),
            (earlier, 0) => format!("+{earlier} earlier To-do items"),
            (earlier, later) => format!("+{earlier} earlier, +{later} later"),
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(theme.text_muted),
        )));
    }
}

fn checklist_window_start(items: &[SidebarWorkChecklistItem], max_items: usize) -> usize {
    if max_items >= items.len() {
        return 0;
    }
    let Some(active_idx) = items
        .iter()
        .position(|item| item.status == TodoStatus::InProgress)
    else {
        return 0;
    };
    active_idx
        .saturating_sub(max_items / 2)
        .min(items.len().saturating_sub(max_items))
}

#[must_use]
fn work_panel_empty_hint(content_width: usize) -> String {
    truncate_line_to_width("No active work", content_width)
}

fn label_with_stop_target(label: &str, content_width: usize) -> String {
    if content_width == 0 {
        return String::new();
    }
    let suffix_width = unicode_width::UnicodeWidthStr::width(TASK_STOP_TARGET_SUFFIX);
    if content_width <= suffix_width {
        return truncate_line_to_width(TASK_STOP_TARGET_LABEL, content_width);
    }
    let base = truncate_line_to_width(label, content_width.saturating_sub(suffix_width));
    format!("{base}{TASK_STOP_TARGET_SUFFIX}")
}

/// Minimal projection of the data the sub-agent sidebar needs. Lifted out
/// of `render_sidebar_subagents` so the rendering can be snapshot-tested
/// without a full `App`.
#[derive(Debug, Clone, Default)]
pub struct SidebarSubagentSummary {
    pub cached_total: usize,
    pub cached_running: usize,
    pub progress_only_count: usize,
    pub fanout_total: Option<usize>,
    pub fanout_running: usize,
    pub foreground_rlm_running: bool,
    pub role_counts: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Default)]
pub struct SidebarAgentRow {
    pub id: String,
    pub parent_run_id: Option<String>,
    pub spawn_depth: u32,
    pub name: String,
    pub model: Option<String>,
    pub status: String,
    pub objective: Option<String>,
    pub git_branch: Option<String>,
    pub progress: Option<String>,
    pub steps_taken: u32,
    pub duration_ms: Option<u64>,
    /// A resident transcript currently contains visible exact evidence. This
    /// conservative signal prevents the sidebar from advertising a dead Open.
    pub transcript_available: bool,
    pub expanded: bool,
    /// `(settled, total)` over this row's direct children, when it has any
    /// (#5479). A fan-out parent's own status says nothing about whether the
    /// work it launched is finished; this is the "5/6 agents done" fact the
    /// rail otherwise makes you count by eye. `None` for a leaf.
    pub children_settled: Option<(usize, usize)>,
}

pub(crate) fn foreground_rlm_running(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|entry| {
            matches!(
                entry,
                HistoryCell::Tool(ToolCell::Generic(generic))
                    if matches!(
                        generic.name.as_str(),
                        "rlm_open" | "rlm_eval" | "rlm_configure" | "rlm_close" | "rlm"
                    ) && generic.status == ToolStatus::Running
            )
        })
    })
}

/// The name a sub-agent was dispatched under, when it has one (#5287).
///
/// `SubAgentResult::name` carries the session name, which the manager seeds
/// with the agent id and only replaces when the dispatch supplied a name. An
/// id is a lookup handle, never the identity an operator dispatched by, so it
/// is reported as absent here and the caller falls back to its own chain.
pub(crate) fn dispatched_agent_name(
    agent: &crate::tools::subagent::SubAgentResult,
) -> Option<&str> {
    let name = agent.name.trim();
    (!name.is_empty() && name != agent.agent_id).then_some(name)
}

pub(crate) fn sidebar_agent_rows(app: &App) -> Vec<SidebarAgentRow> {
    let cached_ids: std::collections::HashSet<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    let display_names = localized_whale_display_names(
        app.subagent_cache
            .iter()
            .map(|agent| (agent.agent_id.as_str(), agent.nickname.as_deref())),
        app.ui_locale.tag(),
    );
    let mut rows: Vec<SidebarAgentRow> = app
        .subagent_cache
        .iter()
        .map(|agent| {
            let current_activity = app
                .agent_progress_meta
                .get(&agent.agent_id)
                .and_then(|meta| meta.current_activity.as_ref());
            let progress = current_activity.map(sidebar_current_activity_text);
            // The dispatch name leads (#5287). Generated whales name the
            // agents that have none, locale-derived from the neutral agent
            // id; never replay a persisted label from another language.
            let display_name = dispatched_agent_name(agent)
                .map(str::to_string)
                .or_else(|| {
                    agent
                        .child_route
                        .as_ref()
                        .and_then(|route| route.resolved_profile_id.as_deref())
                        .map(str::trim)
                        .filter(|profile| !profile.is_empty())
                        .map(str::to_string)
                })
                .or_else(|| display_names.get(&agent.agent_id).cloned())
                .or_else(|| app.agent_label_map.get(&agent.agent_id).cloned())
                .unwrap_or_else(|| agent.name.clone());
            SidebarAgentRow {
                id: agent.agent_id.clone(),
                parent_run_id: agent.parent_run_id.clone(),
                spawn_depth: agent.spawn_depth,
                name: display_name,
                model: Some(agent.model.clone()).filter(|model| !model.trim().is_empty()),
                status: current_activity
                    .map(|activity| sidebar_current_activity_status_text(activity.status))
                    .or_else(|| agent.worker_status.map(sidebar_worker_status_text))
                    .unwrap_or_else(|| subagent_status_text(&agent.status))
                    .to_string(),
                objective: Some(agent.assignment.objective.clone())
                    .filter(|objective| !objective.trim().is_empty()),
                git_branch: agent.git_branch.clone(),
                progress,
                steps_taken: agent.steps_taken,
                duration_ms: Some(agent.duration_ms),
                transcript_available: crate::tui::mouse_ui::resident_agent_transcript_available(
                    app,
                    &agent.agent_id,
                ),
                expanded: app.expanded_sidebar_agents.contains(&agent.agent_id),
                // Filled in by `annotate_child_progress` once every row exists.
                children_settled: None,
            }
        })
        .collect();

    rows.extend(
        app.agent_progress
            .iter()
            .filter(|(id, _)| !cached_ids.contains(id.as_str()))
            .map(|(id, _progress)| {
                // Progress-only rows do not carry a generated whale name yet;
                // keep their existing stable Agent-N placeholder until the
                // manager snapshot arrives.
                let display_name = app
                    .agent_label_map
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_else(|| id.clone());
                let meta = app.agent_progress_meta.get(id.as_str());
                let spawn_depth = meta.map(|meta| meta.spawn_depth).unwrap_or_default();
                let current_activity = meta.and_then(|meta| meta.current_activity.as_ref());
                SidebarAgentRow {
                    id: id.clone(),
                    parent_run_id: meta.and_then(|meta| meta.parent_run_id.clone()),
                    spawn_depth,
                    name: display_name,
                    model: meta.and_then(|meta| meta.resolved_model.clone()),
                    status: current_activity
                        .map(|activity| sidebar_current_activity_status_text(activity.status))
                        .unwrap_or(sidebar_worker_status_text(AgentWorkerStatus::Running))
                        .to_string(),
                    objective: None,
                    git_branch: None,
                    progress: current_activity.map(sidebar_current_activity_text),
                    steps_taken: 0,
                    duration_ms: None,
                    transcript_available: crate::tui::mouse_ui::resident_agent_transcript_available(
                        app, id,
                    ),
                    expanded: app.expanded_sidebar_agents.contains(id),
                    children_settled: None,
                }
            }),
    );

    let mut rows = sort_sidebar_agent_rows_as_tree(rows);
    annotate_child_progress(&mut rows);
    rows
}

/// Fill in each row's `children_settled` from its direct children.
///
/// Counted over the rows actually present: a child whose record has aged out of
/// the ledger cannot be counted, and inventing a denominator that included it
/// would misreport progress as worse than it is.
fn annotate_child_progress(rows: &mut [SidebarAgentRow]) {
    let mut totals: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for row in rows.iter() {
        let Some(parent) = row.parent_run_id.as_deref() else {
            continue;
        };
        let entry = totals.entry(parent.to_string()).or_insert((0, 0));
        entry.1 += 1;
        if sidebar_agent_status_is_terminal(row.status.as_str()) {
            entry.0 += 1;
        }
    }
    for row in rows.iter_mut() {
        row.children_settled = totals.get(&row.id).copied();
    }
}

fn sort_sidebar_agent_rows_as_tree(rows: Vec<SidebarAgentRow>) -> Vec<SidebarAgentRow> {
    let known_ids: std::collections::HashSet<String> =
        rows.iter().map(|row| row.id.clone()).collect();
    let mut children: std::collections::HashMap<String, Vec<usize>> =
        std::collections::HashMap::new();
    let mut roots = Vec::new();

    for (idx, row) in rows.iter().enumerate() {
        if let Some(parent) = row.parent_run_id.as_deref()
            && known_ids.contains(parent)
        {
            children.entry(parent.to_string()).or_default().push(idx);
            continue;
        }
        roots.push(idx);
    }

    fn push_tree(
        idx: usize,
        rows: &[SidebarAgentRow],
        children: &std::collections::HashMap<String, Vec<usize>>,
        seen: &mut std::collections::HashSet<usize>,
        order: &mut Vec<usize>,
    ) {
        if !seen.insert(idx) {
            return;
        }
        order.push(idx);
        if let Some(child_indices) = children.get(&rows[idx].id) {
            for child_idx in child_indices {
                push_tree(*child_idx, rows, children, seen, order);
            }
        }
    }

    let mut order = Vec::with_capacity(rows.len());
    let mut seen = std::collections::HashSet::new();
    for idx in roots {
        push_tree(idx, &rows, &children, &mut seen, &mut order);
    }
    for idx in 0..rows.len() {
        push_tree(idx, &rows, &children, &mut seen, &mut order);
    }

    // Materialize by move instead of cloning each row a second time (#3898):
    // `seen` guarantees every index lands in `order` exactly once, so each
    // slot is taken exactly once and no row is dropped.
    let mut slots: Vec<Option<SidebarAgentRow>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|idx| slots[idx].take().expect("each row emitted exactly once"))
        .collect()
}

fn subagent_status_text(status: &SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "running",
        SubAgentStatus::Completed => "done",
        SubAgentStatus::Interrupted(_) => "interrupted",
        SubAgentStatus::Failed(_) => "failed",
        SubAgentStatus::Cancelled => "canceled",
        SubAgentStatus::BudgetExhausted => "budget",
    }
}

fn sidebar_worker_status_text(status: AgentWorkerStatus) -> &'static str {
    match status {
        AgentWorkerStatus::Queued => "queued",
        AgentWorkerStatus::Starting => "starting",
        AgentWorkerStatus::Running => "running",
        AgentWorkerStatus::WaitingForUser => "waiting",
        AgentWorkerStatus::ModelWait => "model wait",
        AgentWorkerStatus::RunningTool => "tool",
        AgentWorkerStatus::Completed => "done",
        AgentWorkerStatus::Failed => "failed",
        AgentWorkerStatus::Cancelled => "canceled",
        AgentWorkerStatus::Interrupted => "interrupted",
    }
}

fn sidebar_current_activity_status_text(status: AgentCurrentActivityStatus) -> &'static str {
    match status {
        AgentCurrentActivityStatus::Queued => "queued",
        AgentCurrentActivityStatus::Starting => "starting",
        AgentCurrentActivityStatus::Running => "running",
        AgentCurrentActivityStatus::ModelWait => "model wait",
        AgentCurrentActivityStatus::RunningTool => "tool",
        AgentCurrentActivityStatus::Waiting => "waiting",
        AgentCurrentActivityStatus::Done => "done",
        AgentCurrentActivityStatus::Failed => "failed",
        AgentCurrentActivityStatus::Canceled => "canceled",
        AgentCurrentActivityStatus::Interrupted => "interrupted",
    }
}

pub(crate) fn cached_agent_activity_is_live(
    app: &App,
    agent: &crate::tools::subagent::SubAgentResult,
) -> bool {
    if let Some(status) = app
        .agent_progress_meta
        .get(&agent.agent_id)
        .and_then(|meta| meta.current_activity.as_ref())
        .map(|activity| activity.status)
    {
        return matches!(
            status,
            AgentCurrentActivityStatus::Queued
                | AgentCurrentActivityStatus::Starting
                | AgentCurrentActivityStatus::Running
                | AgentCurrentActivityStatus::ModelWait
                | AgentCurrentActivityStatus::RunningTool
                | AgentCurrentActivityStatus::Waiting
        );
    }
    if let Some(status) = agent.worker_status {
        return matches!(
            status,
            AgentWorkerStatus::Queued
                | AgentWorkerStatus::Starting
                | AgentWorkerStatus::Running
                | AgentWorkerStatus::WaitingForUser
                | AgentWorkerStatus::ModelWait
                | AgentWorkerStatus::RunningTool
        );
    }
    matches!(agent.status, SubAgentStatus::Running)
}

fn sidebar_current_activity_text(activity: &AgentCurrentActivity) -> String {
    let mut parts = vec![sidebar_current_activity_status_text(activity.status).to_string()];
    if let Some(tool) = activity.current_tool.as_deref() {
        parts.push(tool.to_string());
    }
    if let Some(step) = activity.step {
        parts.push(format!("step {step}"));
    }
    if let Some(detail) = activity.detail.as_deref()
        && detail != parts[0]
    {
        parts.push(detail.to_string());
    }
    parts.join(" · ")
}

/// Build sub-agent sidebar lines from summary + per-agent rows. Used by the
/// rail's Agents panel (`work_surface::panels`) and the snapshot tests in
/// this module.
pub(crate) fn subagent_panel_lines(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    locale: Locale,
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) -> Vec<Line<'static>> {
    subagent_panel_rows(summary, rows, locale, content_width, max_rows, theme).0
}

/// Render an indented sidebar detail line that never exceeds `content_width`
/// display cells, counting the indent itself (#4094). The earlier inline
/// `format!("  {}", truncate(.., width - 2))` overflowed by the indent width at
/// very narrow terminals (`content_width < 3`, where `saturating_sub(2).max(1)`
/// still leaves room for a glyph that the 2-space prefix then pushes past the
/// column). This keeps the whole line — indent included — within the column.
fn indented_detail_line(indent: &str, body: &str, content_width: usize) -> String {
    let indent_width = unicode_width::UnicodeWidthStr::width(indent);
    if content_width <= indent_width {
        // No room for the indent; clip the body to the whole column so we never
        // overflow, even if that means dropping the indent at pathological widths.
        return truncate_line_to_width(body, content_width);
    }
    format!(
        "{indent}{}",
        truncate_line_to_width(body, content_width - indent_width)
    )
}

/// #4094: reference to a worker's transcript projection, surfaced as a
/// `handle_read` handle instead of dumping the (possibly huge) transcript
/// inline — the inline dump is the freeze/emptiness risk this issue tracks.
/// The child transcript is addressable as the `agent:<id>/full_transcript` var
/// handle (see `subagent_session_projection`); its JSON names the private
/// complete artifact, while clicking Open loads that artifact directly.
///
/// Returns `None` for workers that have not produced anything inspectable yet,
/// so an empty transcript is never advertised. This is the one place a raw
/// agent id is intentionally surfaced in the detail panel (cf. #3030): here it
/// is a functional, copyable handle on its own dedicated line, not incidental
/// id noise mixed into the dossier.
fn subagent_output_handle(row: &SidebarAgentRow) -> Option<String> {
    if !row.transcript_available {
        return None;
    }
    Some(format!("agent:{}/full_transcript", row.id))
}

/// Build the Agents panel lines together with a parallel per-line
/// click-action vector (#3028). Agent label rows open the current-session
/// Fleet worker view (`/pod workers`, formerly spelled `/fleet status`
/// before that name moved to the durable ledger in #4022); header, role-mix,
/// detail, and RLM lines are not clickable.
fn subagent_panel_rows(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    _locale: Locale,
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) -> (Vec<Line<'static>>, Vec<Option<SidebarRowAction>>) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));
    let mut actions: Vec<Option<SidebarRowAction>> = Vec::with_capacity(max_rows.max(4));

    let fanout_total = summary.fanout_total.unwrap_or(0);
    if summary.cached_total == 0
        && summary.progress_only_count == 0
        && fanout_total == 0
        && !summary.foreground_rlm_running
    {
        lines.push(Line::from(Span::styled(
            "No agents",
            Style::default().fg(theme.text_muted),
        )));
        actions.push(None);
        return (lines, actions);
    }

    let (live_running, total) = if let Some(total) = summary.fanout_total {
        (summary.fanout_running, total)
    } else {
        (
            summary.cached_running + summary.progress_only_count,
            summary.cached_total + summary.progress_only_count,
        )
    };
    let done = total.saturating_sub(live_running);
    let header = if live_running > 0 {
        vec![
            Span::styled(
                format!("{live_running} running"),
                Style::default().fg(theme.accent_primary).bold(),
            ),
            Span::styled(format!(" / {total}"), Style::default().fg(theme.text_muted)),
        ]
    } else {
        vec![Span::styled(
            format!("{done} done"),
            Style::default().fg(theme.success),
        )]
    };
    // #4094: the running/done status is the single most useful line, so it must
    // never overflow the sidebar at narrow widths. When the two-tone header
    // fits it renders as-is; when the column is too narrow it collapses into one
    // truncated span so the status is clipped, never spilled past the column.
    let header_width: usize = header
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if header_width > content_width.max(1) {
        let flat: String = header.iter().map(|span| span.content.as_ref()).collect();
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&flat, content_width.max(1)),
            Style::default().fg(theme.text_muted),
        )));
    } else {
        lines.push(Line::from(header));
    }
    actions.push(None);

    if !summary.role_counts.is_empty() {
        let mix: Vec<String> = summary
            .role_counts
            .iter()
            .map(|(role, count)| format!("{count} {role}"))
            .collect();
        let role_line = mix.join(" \u{00B7} ");
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&role_line, content_width.max(1)),
            Style::default().fg(theme.text_dim),
        )));
        actions.push(None);
    }

    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let (marker, color) = agent_status_marker(row.status.as_str(), theme);
        let tree_prefix = agent_tree_prefix(row);
        let label = format!(
            "{tree_prefix}{marker} {}",
            sidebar_agent_row_label(row, content_width.max(1))
        );
        let label = if sidebar_agent_status_is_running(row.status.as_str()) {
            label_with_stop_target(&label, content_width.max(1))
        } else {
            truncate_line_to_width(&label, content_width.max(1))
        };
        lines.push(Line::from(Span::styled(label, Style::default().fg(color))));
        actions.push(Some(SidebarRowAction::ToggleAgentDetails {
            agent_id: row.id.clone(),
        }));

        // Auto-collapse finished sub-agents so the sidebar stays compact when
        // work is done or terminally stopped.
        if sidebar_agent_status_is_terminal(row.status.as_str()) && !row.expanded {
            continue;
        }

        if !row.expanded {
            continue;
        }

        if lines.len() >= max_rows {
            break;
        }
        // Expanded detail: a compact but never-empty dossier for the worker
        // (#4094). Status is always shown first so the expanded panel is never
        // blank while a worker is active; objective/elapsed/model/steps/
        // progress/branch follow when known. Raw ids stay out of the compact
        // line (#3030) — the full id remains available in the hover text.
        let mut detail_parts = Vec::new();
        detail_parts.push(row.status.clone());
        if let Some((settled, total)) = row.children_settled {
            detail_parts.push(format!("{settled}/{total} agents done"));
        }
        if let Some(objective) = row.objective.as_deref()
            && !objective.trim().is_empty()
        {
            detail_parts.push(summarize_tool_output(objective));
        }
        if let Some(model) = row.model.as_deref() {
            detail_parts.push(format!("model {model}"));
        }
        if let Some(duration) = row.duration_ms {
            detail_parts.push(crate::elapsed::format_elapsed_ms(duration));
        }
        if row.steps_taken > 0 {
            detail_parts.push(format!("{} step(s)", row.steps_taken));
        }
        if let Some(progress) = row.progress.as_deref()
            && !progress.trim().is_empty()
        {
            detail_parts.push(summarize_tool_output(progress));
        }
        if let Some(branch) = row.git_branch.as_deref() {
            detail_parts.push(format!("branch {branch}"));
        }
        lines.push(Line::from(Span::styled(
            indented_detail_line("  ", &detail_parts.join(" \u{00B7} "), content_width.max(1)),
            Style::default().fg(theme.text_dim),
        )));
        // One agent, one destination (v0.9.7): clicking the expanded dossier
        // opens the agent's transcript directly. The label row above keeps
        // its expand/collapse toggle. The transcript surface stays bounded —
        // #4094's inline-dump freeze risk does not return with this route.
        actions.push(Some(SidebarRowAction::OpenAgentTranscript {
            agent_id: row.id.clone(),
        }));

        // Secondary action: the bounded Agent Details projection. When the
        // worker has inspectable output, keep the handle_read hint so exact
        // bounded slices stay one command away. Guarded by `max_rows` so the
        // panel stays bounded, and width-clamped so narrow terminals never
        // overflow.
        if lines.len() >= max_rows {
            break;
        }
        let details_line = match subagent_output_handle(row) {
            Some(handle) => {
                format!("\u{25B8} details: open \u{00B7} handle_read {handle}")
            }
            None => "\u{25B8} details: open".to_string(),
        };
        lines.push(Line::from(Span::styled(
            indented_detail_line("  ", &details_line, content_width.max(1)),
            Style::default().fg(theme.text_muted),
        )));
        actions.push(Some(SidebarRowAction::OpenAgentDetail {
            agent_id: row.id.clone(),
        }));
    }

    if summary.foreground_rlm_running {
        lines.push(Line::from(vec![
            Span::styled("RLM", Style::default().fg(theme.accent_primary).bold()),
            Span::styled(
                " foreground work active",
                Style::default().fg(theme.text_dim),
            ),
        ]));
        actions.push(None);
    }

    debug_assert_eq!(lines.len(), actions.len());
    (lines, actions)
}

fn agent_tree_prefix(row: &SidebarAgentRow) -> String {
    if row.parent_run_id.is_none() && row.spawn_depth <= 1 {
        return String::new();
    }
    let depth = row.spawn_depth.max(2).saturating_sub(2).min(6);
    format!("{}└─ ", "  ".repeat(depth as usize))
}

fn sidebar_agent_status_is_terminal(status: &str) -> bool {
    matches!(
        status,
        "done" | "canceled" | "failed" | "interrupted" | "budget"
    )
}

fn sidebar_agent_status_is_running(status: &str) -> bool {
    matches!(
        status,
        "running" | "queued" | "starting" | "waiting" | "model wait" | "tool"
    )
}

fn sidebar_agent_row_label(row: &SidebarAgentRow, max_width: usize) -> String {
    let detail = row
        .objective
        .as_deref()
        .filter(|objective| !objective.trim().is_empty())
        .map(summarize_tool_output)
        .or_else(|| {
            // Progress is only a live substitute for a missing objective;
            // terminal rows would resurface stale in-flight detail.
            if sidebar_agent_status_is_terminal(row.status.as_str()) {
                return None;
            }
            row.progress
                .as_deref()
                .filter(|progress| !progress.trim().is_empty())
                .map(summarize_tool_output)
        });
    match detail {
        Some(detail) => truncate_line_to_width(&format!("{} — {}", row.name, detail), max_width),
        None => truncate_line_to_width(&row.name, max_width),
    }
}

fn agent_status_marker(
    status: &str,
    theme: &palette::UiTheme,
) -> (&'static str, ratatui::style::Color) {
    match status {
        "running" => ("[~]", theme.warning),
        "done" => ("[✓]", theme.success),
        "failed" => ("[!]", theme.error_fg),
        "canceled" | "interrupted" => ("[-]", theme.text_muted),
        _ => ("[ ]", theme.text_muted),
    }
}

/// Session-context panel (#504) — consolidated session state overview.
///
/// Surfaces at-a-glance: working set, token usage / context %, running
/// cost, MCP server count, LSP toggle state, cycle count, and memory
/// file size + mtime. Each section is a compact one-liner so the panel
/// reads as a dashboard rather than a scrolling list.
/// Context panel line builder, lifted out of the legacy sidebar's
/// `render_context_panel` for the unified rail (0.9.4): workspace, token
/// usage, session cost, MCP, LSP, and memory rows.
pub(crate) fn context_panel_lines(app: &App, content_width: usize) -> Vec<Line<'static>> {
    let theme = &app.ui_theme;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(8);

    // ── Working set ──────────────────────────────────────────────
    let ws_name = app
        .workspace
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("(root)")
        .to_string();
    lines.push(Line::from(vec![
        Span::styled(
            truncate_line_to_width(&ws_name, content_width.max(1)),
            Style::default().fg(theme.accent_primary).bold(),
        ),
        Span::styled(
            format!("  {}", app.workspace_context.as_deref().unwrap_or("")),
            Style::default().fg(theme.text_dim),
        ),
    ]));

    // ── Token usage ──────────────────────────────────────────────
    // Context % is disclosed in the header; the sidebar keeps the raw token
    // counts for at-a-glance reference without duplicating the bar.
    let total_tokens = app.session.displayed_total_conversation_tokens();
    let window = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
    lines.push(Line::from(Span::styled(
        format!("context: {total_tokens}/{window} tokens"),
        Style::default().fg(theme.text_muted),
    )));

    // ── Session cost ─────────────────────────────────────────────
    let cost_line = context_panel_cost_line(app);
    lines.push(Line::from(Span::styled(
        cost_line,
        Style::default().fg(theme.text_muted),
    )));

    // ── MCP servers ──────────────────────────────────────────────
    if app.mcp_configured_count > 0 {
        let reload_hint = if app.mcp_reload_required {
            " (reload needed)"
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!("mcp: {} server(s){}", app.mcp_configured_count, reload_hint),
            Style::default().fg(theme.text_muted),
        )));
    }

    // ── LSP ──────────────────────────────────────────────────────
    let lsp_label = if app.lsp_enabled { "on" } else { "off" };
    lines.push(Line::from(Span::styled(
        format!("lsp: {lsp_label}"),
        Style::default().fg(theme.text_muted),
    )));

    // ── Memory ───────────────────────────────────────────────────
    if app.use_memory {
        // Cached by `workspace_context::refresh_if_needed` on its TTL tick.
        // This used to `stat` inline, on every frame the panel was visible
        // (#3908). Before the first refresh lands there is nothing to show,
        // which reads the same as an unreadable file.
        let size_hint = app
            .memory_size_hint
            .clone()
            .unwrap_or_else(|| "—".to_string());
        lines.push(Line::from(Span::styled(
            format!("memory: {} ({})", app.memory_path.display(), size_hint),
            Style::default().fg(theme.text_muted),
        )));
    }

    lines
}

fn context_panel_cost_line(app: &App) -> String {
    let displayed_total = app.displayed_session_cost_for_currency(app.cost_currency);
    let chip = app.cumulative_usage_chip();
    match &chip {
        crate::route_billing::UsageChip::Money(_)
            if crate::route_billing::has_priced_metered_basis(
                app.billing_presentation,
                app.api_provider,
                &app.model,
            ) =>
        {
            let session_cost = app.session_cost_for_currency(app.cost_currency);
            let agent_cost = app.subagent_cost_for_currency(app.cost_currency);
            let real_total = session_cost + agent_cost;
            // Only show the additive breakdown when it matches the displayed
            // total; when the high-water mark is in effect (post-reconciliation),
            // the breakdown would not sum to the displayed value (#244).
            if (displayed_total - real_total).abs() < COST_EQ_TOLERANCE {
                format!(
                    "cost: {} (session {} + agents {})",
                    app.format_cost_amount(displayed_total),
                    app.format_cost_amount(session_cost),
                    app.format_cost_amount(agent_cost)
                )
            } else {
                crate::route_billing::format_usage_line(&chip)
            }
        }
        _ => crate::route_billing::format_usage_line(&chip),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SidebarAgentRow, SidebarSubagentSummary, SidebarWorkChecklistItem, SidebarWorkSummary,
        cached_agent_activity_is_live, context_panel_cost_line, sidebar_agent_rows,
        sidebar_work_summary, subagent_output_handle, subagent_panel_lines, subagent_panel_rows,
        work_panel_empty_hint, work_panel_lines,
    };
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::palette;
    use crate::palette::PaletteMode;
    use crate::tools::goal::GoalStatus;
    use crate::tools::todo::TodoStatus;
    use crate::tui::app::{
        AgentCurrentActivity, AgentCurrentActivityStatus, AgentProgressMeta, App,
        SidebarHoverSection, SidebarHoverState, SidebarRowAction, TuiOptions,
    };
    use ratatui::text::Line;
    use std::path::PathBuf;

    fn create_test_app() -> App {
        let options = TuiOptions {
            ..crate::test_support::test_tui_options(PathBuf::from("."))
        };
        let mut app = App::new(options, &Config::default());
        // Legacy strip geometry (see ui.rs); Bottom default has its own tests.
        app.work_surface.placement = crate::tui::work_surface::WorkSurfacePlacement::Top;
        app
    }

    fn lines_to_text(lines: &[Line<'static>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn context_panel_cost_line_shows_na_for_unpriced_zero_cost_model() {
        let mut app = create_test_app();
        app.model = "unknown-provider/unknown-model".to_string();
        app.billing_presentation = crate::route_billing::BillingPresentation::Metered;

        assert_eq!(context_panel_cost_line(&app), "cost: unknown");
    }

    #[test]
    fn context_panel_cost_line_does_not_inherit_api_pricing_for_codex_oauth() {
        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::OpenaiCodex;
        app.model = "gpt-5.5".to_string();
        app.billing_presentation =
            crate::route_billing::BillingPresentation::Subscription("Codex OAuth quota");
        app.accrue_session_cost_estimate(crate::pricing::CostEstimate::usd_only(12.34));

        let line = context_panel_cost_line(&app);
        assert_eq!(line, "usage: Codex OAuth quota");
        assert!(!line.contains('$'), "OAuth must not invent dollars: {line}");
    }

    #[test]
    fn context_panel_cost_line_marks_unpriced_metered_as_unknown() {
        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::NvidiaNim;
        app.model = "deepseek-ai/deepseek-v4-pro".to_string();
        app.billing_presentation = crate::route_billing::BillingPresentation::Metered;

        assert_eq!(context_panel_cost_line(&app), "cost: unknown");
    }

    /// A route priced only in USD, displayed in CNY mode, must show the USD
    /// figure. The alternative — rendering the CNY accumulator, which is a
    /// structural zero for a USD-only route — would report ¥0.00 as if the
    /// turn were free.
    ///
    /// The USD amount and the coverage that qualifies it are recorded through
    /// the same audit production uses. Bumping the raw accumulator instead
    /// would leave the session with money and no evidence of what it covers,
    /// which is a different (and separately tested) state.
    #[test]
    fn context_panel_cost_line_uses_usd_for_usd_only_model_in_cny_mode() {
        let mut app = create_test_app();
        app.model = "kimi-k2.6".to_string();
        // This test is about METERED currency rendering; pin the route class
        // and a metered provider so the session default (which may be a
        // subscription/OAuth route with no API pricing basis) cannot change
        // what is under test (TUI-DOG-010).
        app.api_provider = crate::config::ApiProvider::Moonshot;
        app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
        app.cost_currency = crate::pricing::CostCurrency::Cny;
        app.record_turn_cost_audit(&usd_only_priced_audit(0.42));
        app.accrue_session_cost_estimate(crate::pricing::CostEstimate::usd_only(0.42));

        let line = context_panel_cost_line(&app);

        assert!(line.contains("$0.42"), "expected USD amount, got {line:?}");
        assert!(
            !line.contains('¥'),
            "must not render CNY zero, got {line:?}"
        );
    }

    /// The same session, before any turn has been priced, must not present the
    /// USD accumulator as a CNY figure or as a total. With no coverage at all
    /// there is nothing to report.
    #[test]
    fn context_panel_cost_line_reports_unknown_before_any_turn_is_priced() {
        let mut app = create_test_app();
        app.model = "kimi-k2.6".to_string();
        app.api_provider = crate::config::ApiProvider::Moonshot;
        app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
        app.cost_currency = crate::pricing::CostCurrency::Cny;

        // Money with no audit behind it: the accumulator moved, coverage did
        // not. This is exactly the shape a legacy/unaudited path produces.
        app.accrue_session_cost_estimate(crate::pricing::CostEstimate::usd_only(0.42));

        let line = context_panel_cost_line(&app);
        assert!(
            !line.contains("0.42"),
            "an unqualified accumulator is not a reportable total: {line:?}"
        );
        assert!(!line.contains('¥'), "must not render CNY zero: {line:?}");
    }

    /// A route priced in CNY reports CNY in CNY mode — the fallback to USD is
    /// for USD-only coverage, not a blanket preference for dollars.
    #[test]
    fn context_panel_cost_line_keeps_cny_when_the_route_is_priced_in_cny() {
        let mut app = create_test_app();
        app.model = "deepseek-v4-flash".to_string();
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app.billing_presentation = crate::route_billing::BillingPresentation::Metered;
        app.cost_currency = crate::pricing::CostCurrency::Cny;
        app.record_turn_cost_audit(&dual_currency_priced_audit(0.42, 3.0));
        app.accrue_session_cost_estimate(crate::pricing::CostEstimate {
            usd: 0.42,
            cny: 3.0,
        });

        let line = context_panel_cost_line(&app);
        assert!(line.contains('¥'), "expected a CNY amount, got {line:?}");
        assert!(!line.contains('$'), "must not fall back to USD: {line:?}");
    }

    /// An authoritatively priced turn that cost nothing is a *known* zero, and
    /// is reported separately from a route whose spend could not be
    /// established. Both currencies are checked so neither can be the one that
    /// quietly reports a fabricated zero.
    #[test]
    fn context_panel_cost_line_separates_a_priced_zero_from_unknown_spend() {
        let mut priced_zero = create_test_app();
        priced_zero.api_provider = crate::config::ApiProvider::Deepseek;
        priced_zero.model = "deepseek-v4-flash".to_string();
        priced_zero.billing_presentation = crate::route_billing::BillingPresentation::Metered;
        priced_zero.record_turn_cost_audit(&dual_currency_priced_audit(0.0, 0.0));
        let priced_zero_line = context_panel_cost_line(&priced_zero);

        let mut unknown = create_test_app();
        unknown.api_provider = crate::config::ApiProvider::Deepseek;
        unknown.model = "deepseek-v4-flash".to_string();
        unknown.billing_presentation = crate::route_billing::BillingPresentation::Metered;
        unknown.record_turn_cost_audit(&unpriced_audit());
        let unknown_line = context_panel_cost_line(&unknown);

        assert_eq!(unknown_line, "cost: unknown");
        assert_ne!(
            priced_zero_line, unknown_line,
            "a provider-reported zero must not render as missing data"
        );
    }

    fn usd_only_priced_audit(usd: f64) -> crate::pricing::TurnCostAudit {
        crate::pricing::TurnCostAudit {
            estimate: Some(crate::pricing::CostEstimate::usd_only(usd)),
            provenance: Some(codewhale_config::pricing::PricingProvenance::ModelsDevBundled),
            unpriced_classes: Vec::new(),
            unpriced_reason: None,
            live_pricing_defect: None,
            usd_priced: true,
            cny_priced: false,
        }
    }

    fn dual_currency_priced_audit(usd: f64, cny: f64) -> crate::pricing::TurnCostAudit {
        crate::pricing::TurnCostAudit {
            estimate: Some(crate::pricing::CostEstimate { usd, cny }),
            provenance: Some(codewhale_config::pricing::PricingProvenance::ModelsDevBundled),
            unpriced_classes: Vec::new(),
            unpriced_reason: None,
            live_pricing_defect: None,
            usd_priced: true,
            cny_priced: true,
        }
    }

    fn unpriced_audit() -> crate::pricing::TurnCostAudit {
        crate::pricing::TurnCostAudit {
            estimate: None,
            provenance: None,
            unpriced_classes: Vec::new(),
            unpriced_reason: Some(crate::pricing::UnpricedReason::NoPricingRow),
            live_pricing_defect: None,
            usd_priced: false,
            cny_priced: false,
        }
    }

    #[test]
    fn work_panel_empty_hint_stays_quiet_and_truncates() {
        let hint = work_panel_empty_hint(10);
        assert!(
            hint.chars().count() <= 10,
            "hint width {} > 10: {hint:?}",
            hint.chars().count()
        );
        assert!(
            !hint.contains("update_plan"),
            "hint should be quiet: {hint:?}"
        );
    }

    #[test]
    fn work_panel_renders_checklist_as_primary_progress_surface_while_incomplete() {
        let summary = SidebarWorkSummary {
            checklist_completion_pct: 33,
            checklist_items: vec![
                SidebarWorkChecklistItem {
                    id: 1,
                    content: "Plan it out".to_string(),
                    status: TodoStatus::Completed,
                },
                SidebarWorkChecklistItem {
                    id: 2,
                    content: "Wire the thing".to_string(),
                    status: TodoStatus::InProgress,
                },
                SidebarWorkChecklistItem {
                    id: 3,
                    content: "Run gates".to_string(),
                    status: TodoStatus::Pending,
                },
            ],
            ..SidebarWorkSummary::default()
        };

        let text = lines_to_text(&work_panel_lines(
            &summary,
            80,
            16,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));

        assert!(
            text[0].starts_with("33% settled (1/3)"),
            "checklist should lead: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("[~] #2 Wire")),
            "in-progress checklist item should be visible: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("50% settled")),
            "strategy progress must not render as a second progress bar when checklist exists: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("Strategy"))
                && !text.iter().any(|line| line.contains("route ")),
            "legacy strategy state must not render beside canonical To-do: {text:?}"
        );
    }

    #[test]
    fn work_panel_keeps_active_checklist_item_visible_when_truncated() {
        let summary = SidebarWorkSummary {
            checklist_completion_pct: 38,
            checklist_items: (1..=8)
                .map(|id| SidebarWorkChecklistItem {
                    id,
                    content: format!("Release task {id}"),
                    status: if id <= 3 {
                        TodoStatus::Completed
                    } else if id == 5 {
                        TodoStatus::InProgress
                    } else {
                        TodoStatus::Pending
                    },
                })
                .collect(),
            ..SidebarWorkSummary::default()
        };

        let text = lines_to_text(&work_panel_lines(
            &summary,
            80,
            6,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));

        assert!(
            text.iter()
                .any(|line| line.contains("[~] #5 Release task 5")),
            "active checklist item should stay visible in a short Work panel: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("earlier"))
                || text.iter().any(|line| line.contains("later")),
            "truncation should explain omitted checklist rows: {text:?}"
        );
    }

    #[test]
    fn work_panel_never_renders_legacy_strategy_state() {
        let empty_text = lines_to_text(&work_panel_lines(
            &SidebarWorkSummary::default(),
            80,
            16,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));
        assert!(
            !empty_text.iter().any(|line| line.contains("Strategy")),
            "empty plan state should not show strategy: {empty_text:?}"
        );

        let summary = SidebarWorkSummary::default();
        let text = lines_to_text(&work_panel_lines(
            &summary,
            80,
            16,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));
        assert!(
            !text.iter().any(|line| line.contains("Strategy"))
                && !text
                    .iter()
                    .any(|line| line.contains("High-level sequencing")),
            "legacy plan state must not create a second panel: {text:?}"
        );
    }

    #[test]
    fn metadata_only_plan_does_not_count_as_visible_work_content() {
        use crate::tools::plan::UpdatePlanArgs;

        let mut app = create_test_app();
        {
            let mut plan = app.plan_state.try_lock().expect("plan lock");
            plan.update(UpdatePlanArgs {
                objective: Some("Ship the catalog lane".to_string()),
                critical_files: vec!["provider_lake.rs".to_string()],
                ..UpdatePlanArgs::default()
            });
        }

        let summary = sidebar_work_summary(&mut app);
        assert!(!summary.has_useful_content());
    }

    #[test]
    fn sidebar_work_summary_caches_on_success() {
        let mut app = create_test_app();
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add("cache test".to_string(), TodoStatus::InProgress);
        }

        let summary = sidebar_work_summary(&mut app);

        assert!(!summary.state_updating, "should not be updating");
        assert_eq!(summary.checklist_items.len(), 1);
        assert!(
            app.cached_work_summary.is_some(),
            "cache should be populated"
        );
    }

    #[test]
    fn sidebar_work_summary_falls_back_to_cache_when_todos_lock_busy() {
        let mut app = create_test_app();
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add("will be cached".to_string(), TodoStatus::Completed);
        }
        let _first = sidebar_work_summary(&mut app);
        assert!(app.cached_work_summary.is_some());

        let held_arc = app.todos.clone();
        let _held = held_arc.try_lock().expect("hold todos lock");

        let summary = sidebar_work_summary(&mut app);

        assert!(!summary.state_updating, "should fall back to cache");
        assert!(
            summary
                .checklist_items
                .iter()
                .any(|item| item.content == "will be cached"),
            "cached item should be present"
        );
    }

    #[test]
    fn sidebar_work_summary_returns_updating_when_no_cache_and_locks_busy() {
        let mut app = create_test_app();
        let held_arc = app.todos.clone();
        let _held = held_arc.try_lock().expect("hold todos lock");

        let summary = sidebar_work_summary(&mut app);

        assert!(summary.state_updating, "should be updating without cache");
    }

    #[test]
    fn sidebar_work_summary_keeps_live_fields_on_cache_fallback() {
        let mut app = create_test_app();
        app.goal.objective = Some("test quarry".to_string());
        app.goal.status = GoalStatus::Complete;
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add("item".to_string(), TodoStatus::Pending);
        }
        let _first = sidebar_work_summary(&mut app);

        app.goal.objective = Some("updated quarry".to_string());
        app.goal.status = GoalStatus::Active;
        let held_arc = app.todos.clone();
        let _held = held_arc.try_lock().expect("hold todos lock");

        let summary = sidebar_work_summary(&mut app);

        assert_eq!(summary.goal_objective.as_deref(), Some("updated quarry"));
        assert!(!summary.goal_completed, "verdict should be live");
    }

    #[test]
    fn sidebar_work_summary_uses_paused_goal_objective_when_goal_is_cleared() {
        let mut app = create_test_app();
        app.goal.objective = None;
        app.paused = true;
        app.paused_goal_objective = Some("Scan nested git repositories".to_string());

        let summary = sidebar_work_summary(&mut app);

        assert_eq!(
            summary.goal_objective.as_deref(),
            Some("Scan nested git repositories")
        );
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"));
        assert!(summary.workflow_paused);
    }

    #[test]
    fn sidebar_names_goal_pause_reason() {
        let mut app = create_test_app();
        app.goal.objective = Some("Finish within budget".to_string());
        app.goal.status = GoalStatus::Paused;
        app.goal.pause_reason = Some(crate::tools::goal::GoalPauseReason::BudgetLimit);

        let summary = sidebar_work_summary(&mut app);

        assert_eq!(
            summary.pause_indicator.as_deref(),
            Some("(Paused: budget limit)")
        );
        assert!(summary.workflow_paused);
    }

    #[test]
    fn work_panel_renders_paused_command_goal() {
        let mut app = create_test_app();
        app.goal.objective = None;
        app.paused = false;
        app.paused_goal_objective = Some("Deploy to staging".to_string());

        let summary = sidebar_work_summary(&mut app);
        let text = lines_to_text(&work_panel_lines(
            &summary,
            80,
            8,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));

        assert!(
            text.first().is_some_and(|line| line.contains('⏸')),
            "paused command should use pause icon: {text:?}"
        );
        assert!(
            text.first()
                .is_some_and(|line| line.contains("Deploy to staging")),
            "paused command title should remain visible: {text:?}"
        );
        assert!(
            text.first().is_some_and(|line| line.contains("(Paused)")),
            "paused state should be visible: {text:?}"
        );
    }

    #[test]
    fn navigator_empty_state_says_no_agents() {
        let summary = SidebarSubagentSummary::default();
        let lines = subagent_panel_lines(&summary, &[], Locale::En, 32, 8, &palette::UI_THEME);
        let text = lines_to_text(&lines);
        assert_eq!(text, vec!["No agents".to_string()]);
    }

    #[test]
    fn navigator_uses_fanout_total_when_fanout_has_seeded_slots() {
        let summary = SidebarSubagentSummary {
            cached_total: 1,
            cached_running: 1,
            progress_only_count: 0,
            fanout_total: Some(6),
            fanout_running: 1,
            foreground_rlm_running: false,
            role_counts: std::collections::BTreeMap::new(),
        };

        let text = lines_to_text(&subagent_panel_lines(
            &summary,
            &[],
            Locale::En,
            64,
            8,
            &palette::UI_THEME,
        ));

        assert!(text[0].contains("1 running"), "header: {:?}", text[0]);
        assert!(text[0].contains("/ 6"), "fanout total: {:?}", text[0]);
    }

    #[test]
    fn navigator_settled_state_says_done() {
        let mut role_counts = std::collections::BTreeMap::new();
        role_counts.insert("general".to_string(), 1);
        let summary = SidebarSubagentSummary {
            cached_total: 1,
            cached_running: 0,
            progress_only_count: 0,
            fanout_total: None,
            fanout_running: 0,
            foreground_rlm_running: false,
            role_counts,
        };
        let text = lines_to_text(&subagent_panel_lines(
            &summary,
            &[],
            Locale::En,
            32,
            8,
            &palette::UI_THEME,
        ));
        assert!(text[0].contains("1 done"), "settled header: {:?}", text[0]);
    }

    #[test]
    fn navigator_truncates_long_role_mix_to_content_width() {
        // Build a wide role mix; assert it doesn't blow past content_width.
        let mut role_counts = std::collections::BTreeMap::new();
        for role in ["general", "explore", "plan", "review", "custom", "extra"] {
            role_counts.insert(role.to_string(), 1);
        }
        let summary = SidebarSubagentSummary {
            cached_total: 6,
            cached_running: 6,
            progress_only_count: 0,
            fanout_total: None,
            fanout_running: 0,
            foreground_rlm_running: false,
            role_counts,
        };
        let lines = subagent_panel_lines(&summary, &[], Locale::En, 16, 8, &palette::UI_THEME);
        let role_line: &str = lines[1]
            .spans
            .first()
            .map(|s| s.content.as_ref())
            .unwrap_or("");
        assert!(
            role_line.chars().count() <= 16,
            "role line {role_line:?} exceeded content_width"
        );
    }

    #[test]
    fn navigator_shows_foreground_rlm_work_when_no_subagents_exist() {
        let summary = SidebarSubagentSummary {
            foreground_rlm_running: true,
            ..SidebarSubagentSummary::default()
        };
        let text = lines_to_text(&subagent_panel_lines(
            &summary,
            &[],
            Locale::En,
            64,
            8,
            &palette::UI_THEME,
        ));

        assert!(!text[0].contains("No agents"), "header: {text:?}");
        assert!(
            text.iter()
                .any(|line| line.contains("RLM foreground work active")),
            "RLM work must be visible in Agents panel: {text:?}"
        );
    }

    // ---- Sidebar hover tooltip tests ----

    #[test]
    fn sidebar_hover_state_default_is_empty() {
        let state = SidebarHoverState::default();
        assert!(state.sections.is_empty());
    }

    #[test]
    fn sidebar_hover_section_stores_lines() {
        use ratatui::layout::Rect;
        let section = SidebarHoverSection {
            content_area: Rect::new(1, 1, 38, 8),
            lines: vec!["line 1".to_string(), "line 2".to_string()],
            rows: vec![],
        };
        assert_eq!(section.lines.len(), 2);
        assert_eq!(section.lines[0], "line 1");
        assert!(section.content_area.x > 0);
    }

    #[test]
    fn hover_line_matching_respects_content_area_offset() {
        use ratatui::layout::Rect;
        let section = SidebarHoverSection {
            content_area: Rect::new(62, 2, 36, 6),
            lines: vec![
                "first".to_string(),
                "second".to_string(),
                "third".to_string(),
            ],
            rows: vec![],
        };

        // Mouse within content area, first line
        let line_idx = (2u16.saturating_sub(section.content_area.y)) as usize;
        assert_eq!(section.lines[line_idx], "first");

        // Mouse within content area, second line
        let line_idx = (3u16.saturating_sub(section.content_area.y)) as usize;
        assert_eq!(section.lines[line_idx], "second");

        // Mouse outside content area (above) — row < content_area.y
        assert!((1u16) < section.content_area.y);
    }

    /// Display width of a single rendered sidebar line, styling stripped.
    fn subagent_line_width(line: &Line<'static>) -> usize {
        lines_to_text(std::slice::from_ref(line))
            .first()
            .map(|s| unicode_width::UnicodeWidthStr::width(s.as_str()))
            .unwrap_or(0)
    }

    /// Summary for a single cached worker with an explicit running count.
    fn single_worker_summary(running: usize) -> SidebarSubagentSummary {
        SidebarSubagentSummary {
            cached_total: 1,
            cached_running: running,
            ..SidebarSubagentSummary::default()
        }
    }

    #[test]
    fn subagent_output_handle_gated_on_inspectable_output() {
        // #4094/#2889: lifecycle and step counts are not exact transcript
        // evidence. Only a successfully inspected resident transcript may
        // advertise the explicit Open route.
        let fresh = SidebarAgentRow {
            id: "agent_fresh".to_string(),
            name: "scout".to_string(),
            status: "starting".to_string(),
            steps_taken: 0,
            expanded: true,
            ..SidebarAgentRow::default()
        };
        assert!(
            subagent_output_handle(&fresh).is_none(),
            "a zero-step non-terminal worker must not advertise a handle"
        );

        let working = SidebarAgentRow {
            steps_taken: 4,
            status: "running".to_string(),
            transcript_available: true,
            ..fresh.clone()
        };
        assert_eq!(
            subagent_output_handle(&working).as_deref(),
            Some("agent:agent_fresh/full_transcript"),
            "a worker with exact resident evidence should expose the transcript handle"
        );

        // A terminal state without exact evidence must not look actionable.
        let failed_immediately = SidebarAgentRow {
            steps_taken: 0,
            status: "failed".to_string(),
            ..fresh.clone()
        };
        assert!(
            subagent_output_handle(&failed_immediately).is_none(),
            "a terminal worker without exact evidence must not advertise Open"
        );
    }

    // ── #3030: stable labels instead of raw internal ids ───────────────────

    #[test]
    fn ensure_agent_label_assigns_stable_sequential_labels() {
        let mut app = create_test_app();
        assert_eq!(app.ensure_agent_label("agent_aaa111"), "Agent 1");
        assert_eq!(app.ensure_agent_label("agent_bbb222"), "Agent 2");
        // Re-seeing a known agent keeps its original label.
        assert_eq!(app.ensure_agent_label("agent_aaa111"), "Agent 1");
        assert_eq!(app.agent_counter, 2);
        // Read-only lookup falls back to the raw id for unknown agents.
        assert_eq!(app.agent_display_label("agent_bbb222"), "Agent 2");
        assert_eq!(app.agent_display_label("agent_zzz999"), "agent_zzz999");
    }

    #[test]
    fn ensure_agent_label_prefers_identity_over_the_counter() {
        let mut app = create_test_app();
        let route = |profile: Option<&str>, role: &str| {
            Some(crate::tools::subagent::ChildRouteReceipt {
                requested_type: "custom".to_string(),
                requested_profile: profile.map(str::to_string),
                resolved_profile_id: None,
                profile_origin: None,
                canonical_role: role.to_string(),
                provider_id: "deepseek".to_string(),
                model_id: "deepseek-v4-pro".to_string(),
                route_source: "roster".to_string(),
                requested_reasoning: "inherit".to_string(),
                effective_reasoning: None,
                runtime_version: "test".to_string(),
                runtime_build_sha: "unknown".to_string(),
            })
        };

        let mut named = cached_agent("agent_named", None);
        named.name = "branch-triage".to_string();
        app.subagent_cache.push(named);

        let mut role = cached_agent("agent_role", None);
        role.assignment.role = Some("reviewer".to_string());
        app.subagent_cache.push(role);

        let mut profile = cached_agent("agent_profile", None);
        profile.assignment.role = None;
        profile.child_route = route(Some("release-lead"), "custom");
        app.subagent_cache.push(profile);

        let mut canonical = cached_agent("agent_canonical", None);
        canonical.assignment.role = None;
        canonical.child_route = route(None, "planner");
        app.subagent_cache.push(canonical);

        let mut typed = cached_agent("agent_typed", None);
        typed.assignment.role = None;
        typed.agent_type = crate::tools::subagent::FleetRole::Builder;
        app.subagent_cache.push(typed);

        // The dispatch name leads, annotated with the role when the role is
        // not already part of the name.
        assert_eq!(
            app.ensure_agent_label("agent_named"),
            "branch-triage · worker"
        );
        // Unnamed children are disambiguated per role (each role's counter
        // starts at 1).
        assert_eq!(app.ensure_agent_label("agent_role"), "reviewer · 1");
        assert_eq!(app.ensure_agent_label("agent_profile"), "release-lead · 1");
        assert_eq!(app.ensure_agent_label("agent_canonical"), "planner · 1");
        assert_eq!(app.ensure_agent_label("agent_typed"), "builder · 1");

        // A progress-only agent first seen before its metadata arrives gets a
        // counter placeholder, then upgrades once the identity is observed.
        assert_eq!(app.ensure_agent_label("agent_late"), "Agent 1");
        let mut late = cached_agent("agent_late", None);
        late.assignment.role = Some("verifier".to_string());
        app.subagent_cache.push(late);
        assert_eq!(app.ensure_agent_label("agent_late"), "verifier · 1");
    }

    #[test]
    fn ensure_agent_label_disambiguates_concurrent_same_role_children() {
        let mut app = create_test_app();

        let mut first = cached_agent("agent_builder_a", None);
        first.assignment.role = None;
        first.agent_type = crate::tools::subagent::FleetRole::Builder;
        app.subagent_cache.push(first);

        let mut second = cached_agent("agent_builder_b", None);
        second.assignment.role = None;
        second.agent_type = crate::tools::subagent::FleetRole::Builder;
        app.subagent_cache.push(second);

        assert_eq!(app.ensure_agent_label("agent_builder_a"), "builder · 1");
        assert_eq!(app.ensure_agent_label("agent_builder_b"), "builder · 2");
        // Stability: re-seeing a known builder keeps its assigned label.
        assert_eq!(app.ensure_agent_label("agent_builder_a"), "builder · 1");
        assert_eq!(app.ensure_agent_label("agent_builder_b"), "builder · 2");

        // A different role has its own sequence.
        let mut reviewer = cached_agent("agent_reviewer_a", None);
        reviewer.assignment.role = Some("reviewer".to_string());
        app.subagent_cache.push(reviewer);
        assert_eq!(app.ensure_agent_label("agent_reviewer_a"), "reviewer · 1");
    }

    #[test]
    fn ensure_agent_label_named_child_skips_role_suffix_when_present() {
        let mut app = create_test_app();

        let mut named = cached_agent("agent_named", None);
        named.name = "release-lead".to_string();
        named.assignment.role = None;
        named.child_route = Some(crate::tools::subagent::ChildRouteReceipt {
            requested_type: "custom".to_string(),
            requested_profile: Some("release-lead".to_string()),
            resolved_profile_id: None,
            profile_origin: None,
            canonical_role: "release-lead".to_string(),
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-v4-pro".to_string(),
            route_source: "roster".to_string(),
            requested_reasoning: "inherit".to_string(),
            effective_reasoning: None,
            runtime_version: "test".to_string(),
            runtime_build_sha: "unknown".to_string(),
        });
        app.subagent_cache.push(named);

        // The role is already part of the name, so no duplicate suffix.
        assert_eq!(app.ensure_agent_label("agent_named"), "release-lead");
    }

    fn cached_agent(
        agent_id: &str,
        nickname: Option<&str>,
    ) -> crate::tools::subagent::SubAgentResult {
        crate::tools::subagent::SubAgentResult {
            // An unnamed dispatch: the manager seeds `name` with the agent id
            // and only replaces it when the caller supplied one.
            name: agent_id.to_string(),
            agent_id: agent_id.to_string(),
            context_mode: "fresh".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: crate::tools::subagent::FleetRole::Worker,
            assignment: crate::tools::subagent::SubAgentAssignment {
                objective: "task".to_string(),
                role: Some("worker".to_string()),
            },
            model: String::new(),
            nickname: nickname.map(str::to_string),
            status: crate::tools::subagent::SubAgentStatus::Running,
            worker_status: None,
            runtime_permissions: None,
            parent_run_id: None,
            spawn_depth: 0,
            child_route: None,
            result: None,
            steps_taken: 1,
            checkpoint: None,
            needs_input: None,
            duration_ms: 100,
            started_at: None,
            from_prior_session: false,
        }
    }

    // === #5479: a fan-out parent shows how much of its fan-out is done ===

    #[test]
    fn a_fanout_parent_row_reports_how_many_children_have_settled() {
        let mut app = create_test_app();
        let parent = cached_agent("workflow_parent", None);
        for index in 0..6 {
            let mut child = cached_agent(&format!("child_{index}"), None);
            child.parent_run_id = Some("workflow_parent".to_string());
            child.spawn_depth = 1;
            if index < 5 {
                child.status = crate::tools::subagent::SubAgentStatus::Completed;
                child.worker_status = Some(crate::tools::subagent::AgentWorkerStatus::Completed);
            }
            app.subagent_cache.push(child);
        }
        app.subagent_cache.push(parent);

        let rows = sidebar_agent_rows(&app);
        let parent_row = rows
            .iter()
            .find(|row| row.id == "workflow_parent")
            .expect("parent row");
        assert_eq!(
            parent_row.children_settled,
            Some((5, 6)),
            "the parent's own status says nothing about its fan-out"
        );
        for row in rows.iter().filter(|row| row.id != "workflow_parent") {
            assert_eq!(
                row.children_settled, None,
                "a leaf must not claim a fan-out it does not have"
            );
        }
    }

    #[test]
    fn a_parent_whose_children_aged_out_reports_no_progress_rather_than_zero() {
        // A denominator that counted rows no longer in the ledger would report
        // progress as worse than it is.
        let mut app = create_test_app();
        app.subagent_cache.push(cached_agent("lonely_parent", None));
        let rows = sidebar_agent_rows(&app);
        assert_eq!(rows[0].children_settled, None);
    }

    #[test]
    fn fanout_progress_appears_in_the_expanded_dossier() {
        let mut app = create_test_app();
        let parent = cached_agent("workflow_parent", None);
        let mut child = cached_agent("child_0", None);
        child.parent_run_id = Some("workflow_parent".to_string());
        child.status = crate::tools::subagent::SubAgentStatus::Completed;
        app.subagent_cache.push(child);
        app.subagent_cache.push(parent);
        app.expanded_sidebar_agents
            .insert("workflow_parent".to_string());

        let rows = sidebar_agent_rows(&app);
        let summary = SidebarSubagentSummary {
            cached_total: rows.len(),
            ..Default::default()
        };
        let lines = subagent_panel_lines(
            &summary,
            &rows,
            Locale::En,
            120,
            40,
            &palette::UiTheme::detect(),
        );
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("1/1 agents done"),
            "the fan-out fact must reach the rendered panel:\n{text}"
        );
    }

    #[test]
    fn sidebar_agent_rows_use_worker_status_from_cached_agents() {
        let mut app = create_test_app();
        let mut agent = cached_agent("agent_model_wait", Some("Blue"));
        agent.worker_status = Some(crate::tools::subagent::AgentWorkerStatus::ModelWait);
        app.subagent_cache.push(agent);

        let rows = sidebar_agent_rows(&app);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "model wait");
        assert_eq!(rows[0].progress.as_deref(), None);
    }

    #[test]
    fn sidebar_agent_rows_project_typed_lifecycle_fixtures() {
        let mut app = create_test_app();
        let fixtures = [
            (
                "agent_running",
                "Running",
                crate::tools::subagent::SubAgentStatus::Running,
                crate::tools::subagent::AgentWorkerStatus::RunningTool,
                AgentCurrentActivityStatus::RunningTool,
                "tool",
            ),
            (
                "agent_waiting",
                "Waiting",
                crate::tools::subagent::SubAgentStatus::Interrupted("approval".to_string()),
                crate::tools::subagent::AgentWorkerStatus::WaitingForUser,
                AgentCurrentActivityStatus::Waiting,
                "waiting",
            ),
            (
                "agent_failed",
                "Failed",
                crate::tools::subagent::SubAgentStatus::Failed("verification".to_string()),
                crate::tools::subagent::AgentWorkerStatus::Failed,
                AgentCurrentActivityStatus::Failed,
                "failed",
            ),
            (
                "agent_done",
                "Done",
                crate::tools::subagent::SubAgentStatus::Completed,
                crate::tools::subagent::AgentWorkerStatus::Completed,
                AgentCurrentActivityStatus::Done,
                "done",
            ),
        ];
        for (id, nickname, status, worker_status, activity_status, _) in &fixtures {
            let mut agent = cached_agent(id, Some(nickname));
            agent.status = status.clone();
            agent.worker_status = Some(*worker_status);
            app.subagent_cache.push(agent);
            app.agent_progress_meta.insert(
                (*id).to_string(),
                AgentProgressMeta {
                    current_activity: Some(AgentCurrentActivity::bounded(
                        *activity_status,
                        (*id == "agent_waiting").then_some("approval required".to_string()),
                        (*id == "agent_running").then_some("read_file".to_string()),
                        Some(2),
                    )),
                    ..AgentProgressMeta::default()
                },
            );
        }

        let rows = sidebar_agent_rows(&app);
        for (id, _, _, _, _, expected_status) in fixtures {
            let row = rows
                .iter()
                .find(|row| row.id == id)
                .expect("typed lifecycle row");
            assert_eq!(row.status, expected_status);
        }
        let waiting = app
            .subagent_cache
            .iter()
            .find(|agent| agent.agent_id == "agent_waiting")
            .expect("waiting agent");
        assert!(cached_agent_activity_is_live(&app, waiting));
    }

    #[test]
    fn sidebar_progress_only_rows_never_infer_status_from_display_text() {
        let mut app = create_test_app();
        app.ensure_agent_label("agent_queued");
        app.agent_progress.insert(
            "agent_queued".to_string(),
            "queued waiting failed completed".to_string(),
        );

        let rows = sidebar_agent_rows(&app);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Agent 1");
        assert_eq!(rows[0].status, "running");
        assert_eq!(rows[0].progress, None);

        app.agent_progress_meta.insert(
            "agent_queued".to_string(),
            AgentProgressMeta {
                current_activity: Some(AgentCurrentActivity::bounded(
                    AgentCurrentActivityStatus::Queued,
                    Some("waiting for launch permit".to_string()),
                    None,
                    None,
                )),
                ..AgentProgressMeta::default()
            },
        );
        crate::tui::ui::record_agent_spawned_route(&mut app, "agent_queued", "deepseek-v4-pro");
        let rows = sidebar_agent_rows(&app);
        assert_eq!(rows[0].status, "queued");
        assert_eq!(rows[0].model.as_deref(), Some("deepseek-v4-pro"));
        assert_eq!(
            rows[0].progress.as_deref(),
            Some("queued · waiting for launch permit")
        );
    }

    #[test]
    fn sidebar_agent_rows_preserve_explicit_names_and_derive_whales_from_locale() {
        let mut app = create_test_app();
        let agent_id = "agent_cafe0123";
        app.ensure_agent_label(agent_id);
        app.subagent_cache
            .push(cached_agent(agent_id, Some("doc-fixer")));

        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(
            rows[0].name, "doc-fixer",
            "an explicit custom nickname remains user-owned"
        );

        // Without an explicit nickname, display is derived from the neutral id
        // in the active UI locale rather than from the old Agent-N label.
        app.subagent_cache[0].nickname = None;
        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(
            rows[0].name,
            crate::tools::subagent::whale_name_for_id_in_locale(agent_id, "en")
        );
    }

    #[test]
    fn sidebar_agent_rows_lead_with_the_dispatch_name() {
        // #5287: operators dispatch by name and think by name, so the session
        // name outranks both the generated whale and the Agent-N label.
        let mut app = create_test_app();
        let agent_id = "agent_cafe0123";
        app.ensure_agent_label(agent_id);
        let mut agent = cached_agent(agent_id, Some("Blue Whale"));
        agent.name = "branch-triage".to_string();
        app.subagent_cache.push(agent);

        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(rows[0].name, "branch-triage");
    }

    #[test]
    fn sidebar_agent_rows_prefer_resolved_profile_over_generated_whale() {
        let mut app = create_test_app();
        let agent_id = "agent_cafe0123";
        app.ensure_agent_label(agent_id);
        let mut agent = cached_agent(agent_id, Some("Blue Whale"));
        agent.child_route = Some(crate::tools::subagent::ChildRouteReceipt {
            requested_type: "custom".to_string(),
            requested_profile: Some("DeepSeek V4 Flash".to_string()),
            resolved_profile_id: Some("flash-scout".to_string()),
            profile_origin: Some("fleet:release".to_string()),
            canonical_role: "scout".to_string(),
            provider_id: "deepseek".to_string(),
            model_id: "deepseek-v4-flash-vision-exp".to_string(),
            route_source: "fleet".to_string(),
            requested_reasoning: "inherit".to_string(),
            effective_reasoning: None,
            runtime_version: "test".to_string(),
            runtime_build_sha: "unknown".to_string(),
        });
        app.subagent_cache.push(agent);

        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(rows[0].name, "flash-scout");
    }

    #[test]
    fn english_sidebar_relocalizes_mixed_persisted_whale_names() {
        let mut app = create_test_app();
        app.ui_locale = Locale::En;
        for (agent_id, legacy_locale) in [
            ("agent_locale_a", "zh-Hans"),
            ("agent_locale_b", "ja"),
            ("agent_locale_c", "vi"),
        ] {
            let legacy_name =
                crate::tools::subagent::whale_name_for_id_in_locale(agent_id, legacy_locale);
            app.subagent_cache
                .push(cached_agent(agent_id, Some(&legacy_name)));
        }

        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert!(
                row.name.is_ascii(),
                "English Fleet display leaked a prior-locale whale: {}",
                row.name
            );
            assert_eq!(
                row.name,
                crate::tools::subagent::whale_name_for_id_in_locale(&row.id, "en")
            );
        }
    }

    // --- Unicode / CJK / terminal-width QA (issue #3488) -------------------
    // The sub-agent overlay renders CJK display names next to ASCII ids,
    // numeric columns (step count, elapsed), status verbs, and branch lines.
    // These guard that a CJK name never shifts the status columns, corrupts the
    // panel border, or hides the running/completed state (#3488 dogfood case:
    // a worker named 抹香鲸).

    /// Build the exact dogfood fixture: a CJK-named running implementer with a
    /// mixed English/CJK objective, a long branch, step count, and elapsed time.
    fn cjk_running_implementer_row() -> SidebarAgentRow {
        SidebarAgentRow {
            id: "agent_e0b2dcf1".to_string(),
            parent_run_id: None,
            spawn_depth: 1,
            name: "抹香鲸".to_string(),
            model: Some("glm-5.2".to_string()),
            status: "running".to_string(),
            objective: Some(
                "QUESTION: Add Zhipu GLM as a first-class provider-scoped model (issue #3439)"
                    .to_string(),
            ),
            git_branch: Some("codex/issue-3439-zhipu-glm-fixture".to_string()),
            progress: Some("step 10: finished tool edit_file ok".to_string()),
            steps_taken: 10,
            duration_ms: Some(124_838),
            transcript_available: false,
            expanded: true,
            children_settled: None,
        }
    }

    #[test]
    fn subagent_panel_cjk_display_name_keeps_columns_and_state_at_narrow_and_medium_widths() {
        let summary = single_worker_summary(1);
        let rows = vec![cjk_running_implementer_row()];

        // Across pathological single-cell widths up through a medium terminal,
        // every rendered line (count header, role-mix, label, dossier, handle)
        // must stay within the column budget by *display* width and never split
        // a wide glyph into a replacement char — which is what would corrupt the
        // panel border or visually drift the status columns.
        for content_width in [1usize, 2, 3, 5, 8, 12, 16, 20, 24, 40, 80] {
            let (lines, actions) = subagent_panel_rows(
                &summary,
                &rows,
                Locale::En,
                content_width,
                8,
                &palette::UI_THEME,
            );
            assert_eq!(lines.len(), actions.len(), "width {content_width}");
            for line in &lines {
                assert!(
                    subagent_line_width(line) <= content_width,
                    "width {content_width}: line overflows by display width ({} cells)",
                    subagent_line_width(line)
                );
                let text = lines_to_text(std::slice::from_ref(line)).join("");
                assert!(
                    !text.contains('\u{FFFD}'),
                    "width {content_width}: wide glyph split during truncation: {text:?}"
                );
            }
        }

        // At medium/usable widths the CJK name must not hide the running state:
        // the status marker `[~]`, the compact stop target `[x]`, and the CJK
        // display name all survive, and the row still resolves to its agent id.
        for content_width in [40usize, 80] {
            let (lines, actions) = subagent_panel_rows(
                &summary,
                &rows,
                Locale::En,
                content_width,
                8,
                &palette::UI_THEME,
            );
            let text = lines_to_text(&lines);

            let label_idx = text
                .iter()
                .position(|line| line.contains("抹香鲸"))
                .unwrap_or_else(|| {
                    panic!("width {content_width}: CJK display name dropped: {text:?}")
                });
            assert!(
                text[label_idx].contains("[~]"),
                "width {content_width}: running marker hidden by CJK name: {text:?}"
            );
            assert!(
                text[label_idx].ends_with("[x]"),
                "width {content_width}: stop target hidden by CJK name: {text:?}"
            );
            assert!(
                !text[label_idx].contains('\u{FFFD}'),
                "width {content_width}: CJK name split: {text:?}"
            );
            assert!(
                matches!(
                    actions[label_idx],
                    Some(SidebarRowAction::ToggleAgentDetails { ref agent_id })
                        if agent_id == "agent_e0b2dcf1"
                ),
                "width {content_width}: CJK row must still resolve to its agent id"
            );
        }
    }

    /// One agent, one destination (v0.9.7): the expanded dossier line opens
    /// the agent's transcript directly; the bounded Agent Details projection
    /// demotes to the labelled secondary line beneath it. With exact resident
    /// evidence the secondary line keeps the copyable `handle_read` hint.
    #[test]
    fn expanded_dossier_opens_transcript_and_details_line_is_secondary() {
        let summary = single_worker_summary(1);
        let mut row = cjk_running_implementer_row();
        row.expanded = true;

        for transcript_available in [false, true] {
            row.transcript_available = transcript_available;
            let (lines, actions) = subagent_panel_rows(
                &summary,
                &[row.clone()],
                Locale::En,
                80,
                8,
                &palette::UI_THEME,
            );
            let text = lines_to_text(&lines);

            // The dossier is the indented detail line that leads with the
            // status word (its tail is width-truncated, so no tail anchor).
            let dossier_idx = text
                .iter()
                .position(|line| line.trim_start().starts_with("running \u{00B7}"))
                .expect("expanded dossier line");
            assert!(
                matches!(
                    actions[dossier_idx],
                    Some(SidebarRowAction::OpenAgentTranscript { ref agent_id })
                        if agent_id == "agent_e0b2dcf1"
                ),
                "dossier click must open the transcript: {text:?}"
            );

            let details_idx = text
                .iter()
                .position(|line| line.contains("details: open"))
                .expect("secondary details line");
            assert!(
                matches!(
                    actions[details_idx],
                    Some(SidebarRowAction::OpenAgentDetail { ref agent_id })
                        if agent_id == "agent_e0b2dcf1"
                ),
                "details line must open the bounded projection: {text:?}"
            );
            assert_eq!(
                text[details_idx].contains("handle_read"),
                transcript_available,
                "handle hint must track exact evidence: {text:?}"
            );
        }
    }
}
