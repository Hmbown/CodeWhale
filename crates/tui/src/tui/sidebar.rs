//! Sidebar rendering — Work / Tasks / Agents / Context panels.
//!
//! Extracted from `tui/ui.rs` (P1.2). The sidebar appears to the right of
//! the chat transcript when the available width allows it. Each section
//! reads from `App` snapshots; mutation lives in the main app loop.

use std::fmt::Write;
use std::time::Duration;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    prelude::Widget,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::deepseek_theme::Theme;
use crate::palette;
use crate::tools::plan::StepStatus;
use crate::tools::subagent::SubAgentStatus;
use crate::tools::todo::TodoStatus;

use super::app::{App, SidebarFocus, TaskPanelEntry};
use super::history::{GenericToolCell, HistoryCell, ToolCell, ToolStatus, summarize_tool_output};
use super::subagent_routing::active_fanout_counts;
use super::ui_text::{concise_shell_command_label, truncate_line_to_width};

/// Tolerance for floating-point cost comparison in the sidebar breakdown.
/// Must be large enough that accumulated f64 error across hundreds of turns
/// does not prematurely hide the session+agents breakdown.
const COST_EQ_TOLERANCE: f64 = 1e-6;
const RECENT_TOOL_SCAN_LIMIT: usize = 24;
const ACTIVE_TOOL_COMPLETED_ROW_TTL: Duration = Duration::from_secs(8);
const ACTIVE_TOOL_STALE_RUNNING_ROW_TTL: Duration = Duration::from_secs(600);

pub fn render_sidebar(f: &mut Frame, area: Rect, app: &App) {
    if area.width < 24 || area.height < 8 {
        // Paint a styled block over the area so stale cells from a previous
        // (wider) frame don't persist as bleed-through artifacts (#400).
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(area, f.buffer_mut());
        return;
    }

    match app.sidebar_focus {
        SidebarFocus::Auto => render_sidebar_auto(f, area, app),
        SidebarFocus::Work => render_sidebar_work(f, area, app),
        SidebarFocus::Tasks => render_sidebar_tasks(f, area, app),
        SidebarFocus::Agents => render_sidebar_subagents(f, area, app),
        SidebarFocus::Context => render_context_panel(f, area, app),
        SidebarFocus::Hidden => Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(area, f.buffer_mut()),
    }
}

/// Build the Auto-mode panel stack. Empty panels collapse to zero height so
/// non-empty ones get the full sidebar real estate. Work appears when it has
/// useful content, or as the one quiet empty state when nothing else is active.
fn render_sidebar_auto(f: &mut Frame, area: Rect, app: &App) {
    let work_has_content = sidebar_work_summary(app).has_useful_content();
    let tasks_empty = app.runtime_turn_id.is_none() && app.task_panel.is_empty();
    let agents_empty = app.subagent_cache.is_empty()
        && app.agent_progress.is_empty()
        && active_fanout_counts(app).is_none()
        && !foreground_rlm_running(app);

    let visible = auto_sidebar_panels(AutoSidebarState {
        work_has_content,
        tasks_empty,
        agents_empty,
        context_enabled: app.context_panel,
    });

    let constraints: Vec<Constraint> = match visible.len() {
        1 => vec![Constraint::Min(0)],
        2 => vec![Constraint::Percentage(50), Constraint::Min(0)],
        3 => vec![
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Min(0),
        ],
        4 => vec![
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Min(6),
        ],
        _ => vec![
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Percentage(20),
            Constraint::Min(6),
        ],
    };

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    for (panel, rect) in visible.iter().zip(sections.iter()) {
        match panel {
            AutoSidebarPanel::Work => render_sidebar_work(f, *rect, app),
            AutoSidebarPanel::Tasks => render_sidebar_tasks(f, *rect, app),
            AutoSidebarPanel::Agents => render_sidebar_subagents(f, *rect, app),
            AutoSidebarPanel::Context => render_context_panel(f, *rect, app),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoSidebarPanel {
    Work,
    Tasks,
    Agents,
    Context,
}

#[derive(Debug, Clone, Copy)]
struct AutoSidebarState {
    work_has_content: bool,
    tasks_empty: bool,
    agents_empty: bool,
    context_enabled: bool,
}

fn auto_sidebar_panels(state: AutoSidebarState) -> Vec<AutoSidebarPanel> {
    let nothing_else_active = state.tasks_empty && state.agents_empty && !state.context_enabled;
    let mut visible = Vec::with_capacity(4);

    if state.work_has_content || nothing_else_active {
        visible.push(AutoSidebarPanel::Work);
    }
    if !state.tasks_empty {
        visible.push(AutoSidebarPanel::Tasks);
    }
    if !state.agents_empty {
        visible.push(AutoSidebarPanel::Agents);
    }
    if state.context_enabled {
        visible.push(AutoSidebarPanel::Context);
    }

    visible
}

#[derive(Debug, Clone)]
struct SidebarWorkChecklistItem {
    id: u32,
    content: String,
    status: TodoStatus,
}

#[derive(Debug, Clone)]
struct SidebarWorkStrategyStep {
    text: String,
    status: StepStatus,
    elapsed: String,
}

#[derive(Debug, Clone, Default)]
struct SidebarWorkSummary {
    goal_objective: Option<String>,
    goal_token_budget: Option<u32>,
    tokens_used: u32,
    cycle_count: u32,
    checklist_completion_pct: u8,
    checklist_items: Vec<SidebarWorkChecklistItem>,
    strategy_explanation: Option<String>,
    strategy_steps: Vec<SidebarWorkStrategyStep>,
    state_updating: bool,
}

impl SidebarWorkSummary {
    fn has_strategy(&self) -> bool {
        self.strategy_explanation
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || !self.strategy_steps.is_empty()
    }

    fn has_useful_content(&self) -> bool {
        self.goal_objective
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
            || self.cycle_count > 0
            || !self.checklist_items.is_empty()
            || self.has_strategy()
            || self.state_updating
    }

    fn strategy_counts(&self) -> (usize, usize, usize) {
        let mut pending = 0;
        let mut in_progress = 0;
        let mut completed = 0;
        for step in &self.strategy_steps {
            match step.status {
                StepStatus::Pending => pending += 1,
                StepStatus::InProgress => in_progress += 1,
                StepStatus::Completed => completed += 1,
            }
        }
        (pending, in_progress, completed)
    }

    fn strategy_progress_percent(&self) -> u8 {
        if self.strategy_steps.is_empty() {
            return 0;
        }
        let completed = self
            .strategy_steps
            .iter()
            .filter(|step| step.status == StepStatus::Completed)
            .count();
        let percent = completed.saturating_mul(100) / self.strategy_steps.len();
        u8::try_from(percent).unwrap_or(u8::MAX)
    }
}

fn sidebar_work_summary(app: &App) -> SidebarWorkSummary {
    let mut summary = SidebarWorkSummary {
        goal_objective: app.goal.goal_objective.clone(),
        goal_token_budget: app.goal.goal_token_budget,
        tokens_used: app.session.total_conversation_tokens,
        cycle_count: app.cycle_count,
        ..SidebarWorkSummary::default()
    };

    match app.todos.try_lock() {
        Ok(todos) => {
            let snapshot = todos.snapshot();
            summary.checklist_completion_pct = snapshot.completion_pct;
            summary.checklist_items = snapshot
                .items
                .into_iter()
                .map(|item| SidebarWorkChecklistItem {
                    id: item.id,
                    content: item.content,
                    status: item.status,
                })
                .collect();
        }
        Err(_) => {
            summary.state_updating = true;
        }
    }

    match app.plan_state.try_lock() {
        Ok(plan) => {
            if !plan.is_empty() {
                summary.strategy_explanation = plan.explanation().map(str::to_string);
                summary.strategy_steps = plan
                    .steps()
                    .iter()
                    .map(|step| SidebarWorkStrategyStep {
                        text: step.text.clone(),
                        status: step.status.clone(),
                        elapsed: step.elapsed_str(),
                    })
                    .collect();
            }
        }
        Err(_) => {
            summary.state_updating = true;
        }
    }

    summary
}

fn work_panel_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    palette_mode: palette::PaletteMode,
) -> Vec<Line<'static>> {
    let theme = Theme::for_palette_mode(palette_mode);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    push_work_goal_lines(summary, content_width, max_rows, &mut lines);

    if summary.state_updating && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            "Work state updating...",
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    push_work_checklist_lines(summary, content_width, max_rows, &mut lines);
    push_work_strategy_lines(summary, content_width, max_rows, &mut lines, &theme);

    if summary.cycle_count > 0 && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            format!(
                "cycles: {} (active: {})",
                summary.cycle_count,
                summary.cycle_count.saturating_add(1)
            ),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            work_panel_empty_hint(content_width),
            Style::default().fg(palette::TEXT_MUTED).italic(),
        )));
    }

    lines
}

fn push_work_goal_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
) {
    let Some(objective) = summary.goal_objective.as_deref() else {
        return;
    };
    if objective.trim().is_empty() || lines.len() >= max_rows {
        return;
    }

    lines.push(Line::from(Span::styled(
        format!(
            "◆ {}",
            truncate_line_to_width(objective, content_width.saturating_sub(2).max(1))
        ),
        Style::default()
            .fg(palette::STATUS_WARNING)
            .add_modifier(ratatui::style::Modifier::BOLD),
    )));

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
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }
}

fn push_work_checklist_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
) {
    if summary.checklist_items.is_empty() || lines.len() >= max_rows {
        return;
    }

    let total = summary.checklist_items.len();
    let completed = summary
        .checklist_items
        .iter()
        .filter(|item| item.status == TodoStatus::Completed)
        .count();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}%", summary.checklist_completion_pct),
            Style::default().fg(palette::STATUS_SUCCESS).bold(),
        ),
        Span::styled(
            format!(" complete ({completed}/{total})"),
            Style::default().fg(palette::TEXT_MUTED),
        ),
    ]));

    let reserve_for_strategy = if summary.has_strategy() { 2 } else { 0 };
    let available_item_rows = max_rows
        .saturating_sub(lines.len())
        .saturating_sub(reserve_for_strategy)
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
        let (prefix, color) = match item.status {
            TodoStatus::Pending => ("[ ]", palette::TEXT_MUTED),
            TodoStatus::InProgress => ("[~]", palette::STATUS_WARNING),
            TodoStatus::Completed => ("[x]", palette::STATUS_SUCCESS),
        };
        let text = format!("{prefix} #{} {}", item.id, item.content);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&text, content_width),
            Style::default().fg(color),
        )));
    }

    let earlier = start;
    let later = summary.checklist_items.len().saturating_sub(end);
    let remaining = earlier.saturating_add(later);
    if remaining > 0 && lines.len() < max_rows {
        let label = match (earlier, later) {
            (0, later) => format!("+{later} more checklist items"),
            (earlier, 0) => format!("+{earlier} earlier checklist items"),
            (earlier, later) => format!("+{earlier} earlier, +{later} later"),
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(palette::TEXT_MUTED),
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

fn push_work_strategy_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
    theme: &Theme,
) {
    if !summary.has_strategy() || lines.len() >= max_rows {
        return;
    }

    if summary.checklist_items.is_empty() && !summary.strategy_steps.is_empty() {
        let (pending, in_progress, completed) = summary.strategy_counts();
        let total = pending + in_progress + completed;
        lines.push(Line::from(vec![
            Span::styled(
                "Strategy ",
                Style::default().fg(theme.plan_summary_color).bold(),
            ),
            Span::styled(
                format!("{}%", summary.strategy_progress_percent()),
                Style::default().fg(theme.plan_progress_color).bold(),
            ),
            Span::styled(
                format!(" complete ({completed}/{total})"),
                Style::default().fg(theme.plan_summary_color),
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Strategy",
            Style::default().fg(theme.plan_summary_color).bold(),
        )));
    }

    if let Some(explanation) = summary.strategy_explanation.as_deref()
        && lines.len() < max_rows
    {
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(explanation, content_width),
            Style::default().fg(theme.plan_explanation_color),
        )));
    }

    let max_steps = max_rows
        .saturating_sub(lines.len())
        .min(summary.strategy_steps.len());
    for step in summary.strategy_steps.iter().take(max_steps) {
        let (prefix, color) = match step.status {
            StepStatus::Pending => ("[ ]", theme.plan_pending_color),
            StepStatus::InProgress => ("[~]", theme.plan_in_progress_color),
            StepStatus::Completed => ("[x]", theme.plan_completed_color),
        };
        let mut text = format!("{prefix} {}", step.text);
        if !step.elapsed.is_empty() {
            let _ = write!(text, " ({})", step.elapsed);
        }
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&text, content_width),
            Style::default().fg(color),
        )));
    }

    let remaining = summary.strategy_steps.len().saturating_sub(max_steps);
    if remaining > 0 && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            format!("+{remaining} more strategy steps"),
            Style::default().fg(theme.plan_summary_color),
        )));
    }
}

#[must_use]
fn work_panel_empty_hint(content_width: usize) -> String {
    truncate_line_to_width("No active work", content_width)
}

fn render_sidebar_work(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let usable_rows = area.height.saturating_sub(3) as usize;
    let summary = sidebar_work_summary(app);
    let lines = work_panel_lines(
        &summary,
        content_width.max(1),
        usable_rows,
        app.ui_theme.mode,
    );

    render_sidebar_section(f, area, "Work", lines, app);
}

fn render_sidebar_tasks(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let usable_rows = area.height.saturating_sub(3) as usize;
    let lines = task_panel_lines(app, content_width.max(1), usable_rows.max(1));

    render_sidebar_section(f, area, "Tasks", lines, app);
}

#[derive(Debug, Clone)]
struct SidebarToolRow {
    name: String,
    status: ToolStatus,
    summary: String,
    duration_ms: Option<u64>,
}

fn task_panel_lines(app: &App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = app
            .runtime_turn_status
            .as_deref()
            .unwrap_or("unknown")
            .to_string();
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(
                &format!("turn {} ({status})", truncate_line_to_width(turn_id, 12)),
                content_width.max(1),
            ),
            Style::default().fg(palette::DEEPSEEK_SKY),
        )));
    }

    let active_rows = active_tool_rows(app);
    if !active_rows.is_empty() && lines.len() < max_rows {
        push_sidebar_label(&mut lines, "Live tools", palette::DEEPSEEK_SKY);
        push_tool_rows(&mut lines, &active_rows, content_width, max_rows);
    }

    let background_rows = background_task_rows(app, &active_rows);
    if !background_rows.is_empty() && lines.len() < max_rows {
        let running = background_rows
            .iter()
            .filter(|task| task.status == "running")
            .count();
        lines.push(Line::from(vec![
            Span::styled(
                if running == background_rows.len() {
                    format!("Background jobs: {running} running")
                } else {
                    format!("Background jobs: {} active", background_rows.len())
                },
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            ),
            Span::styled(
                if running == background_rows.len() {
                    String::new()
                } else {
                    format!(" ({running} running)")
                },
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]));

        let max_items = max_rows.saturating_sub(lines.len());
        for task in background_rows.iter().take(max_items) {
            let color = match task.status.as_str() {
                "queued" => palette::TEXT_MUTED,
                "running" => palette::STATUS_WARNING,
                "completed" => palette::STATUS_SUCCESS,
                "failed" => palette::STATUS_ERROR,
                "canceled" => palette::TEXT_DIM,
                _ => palette::TEXT_MUTED,
            };
            let duration = task
                .duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string());
            let label = format!(
                "{} {} {}",
                truncate_line_to_width(&task.id, 10),
                task.status,
                duration
            );
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&label, content_width.max(1)),
                Style::default().fg(color),
            )));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_line_to_width(
                        &task.prompt_summary,
                        content_width.saturating_sub(2).max(1)
                    )
                ),
                Style::default().fg(palette::TEXT_DIM),
            )));
        }

        if lines.len() < max_rows
            && background_rows
                .iter()
                .any(|task| task.id.starts_with("shell_") && task.status == "running")
        {
            lines.push(Line::from(Span::styled(
                truncate_line_to_width("Ctrl+K -> /jobs cancel-all", content_width.max(1)),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(ratatui::style::Modifier::ITALIC),
            )));
        }
    }

    if lines.len() < max_rows {
        let recent_rows = recent_tool_rows(app, 4);
        if !recent_rows.is_empty() {
            push_sidebar_label(&mut lines, "Recent tools", palette::TEXT_DIM);
            push_tool_rows(&mut lines, &recent_rows, content_width, max_rows);
        }
    }

    if lines.is_empty()
        || (lines.len() == 1
            && app.runtime_turn_id.is_some()
            && active_rows.is_empty()
            && background_rows.is_empty())
    {
        lines.push(Line::from(Span::styled(
            "No live tools or background jobs",
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    lines
}

fn push_sidebar_label(lines: &mut Vec<Line<'static>>, label: &str, color: ratatui::style::Color) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(color).bold(),
    )));
}

fn active_tool_rows(app: &App) -> Vec<SidebarToolRow> {
    let Some(active) = app.active_cell.as_ref() else {
        return Vec::new();
    };
    let mut rows: Vec<SidebarToolRow> = Vec::new();
    let mut stale_running: Vec<SidebarToolRow> = Vec::new();
    for (entry_idx, cell) in active.entries().iter().enumerate() {
        let Some(row) = sidebar_tool_row_from_cell(cell) else {
            continue;
        };
        match active_tool_row_visibility(app, entry_idx, &row) {
            ActiveToolRowVisibility::Visible => rows.push(row),
            ActiveToolRowVisibility::StaleRunning => stale_running.push(row),
            ActiveToolRowVisibility::Hidden => {}
        }
    }
    if !stale_running.is_empty() {
        rows.push(collapsed_stale_running_row(stale_running));
    }
    editorial_tool_rows(rows, usize::MAX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveToolRowVisibility {
    Visible,
    StaleRunning,
    Hidden,
}

fn active_tool_row_visibility(
    app: &App,
    entry_idx: usize,
    row: &SidebarToolRow,
) -> ActiveToolRowVisibility {
    if row.status == ToolStatus::Running {
        return if row
            .duration_ms
            .is_some_and(|ms| ms >= duration_ms(ACTIVE_TOOL_STALE_RUNNING_ROW_TTL))
        {
            ActiveToolRowVisibility::StaleRunning
        } else {
            ActiveToolRowVisibility::Visible
        };
    }

    let Some(completed_at) = app.active_tool_entry_completed_at.get(&entry_idx) else {
        return ActiveToolRowVisibility::Hidden;
    };
    if completed_at.elapsed() <= ACTIVE_TOOL_COMPLETED_ROW_TTL {
        ActiveToolRowVisibility::Visible
    } else {
        ActiveToolRowVisibility::Hidden
    }
}

fn collapsed_stale_running_row(rows: Vec<SidebarToolRow>) -> SidebarToolRow {
    let count = rows.len();
    let oldest_ms = rows
        .iter()
        .filter_map(|row| row.duration_ms)
        .max()
        .unwrap_or_default();
    let first_summary = rows
        .iter()
        .find_map(|row| (!row.summary.trim().is_empty()).then(|| row.summary.clone()))
        .unwrap_or_else(|| "open Activity Detail".to_string());
    SidebarToolRow {
        name: if count == 1 {
            "run".to_string()
        } else {
            format!("run x{count}")
        },
        status: ToolStatus::Running,
        summary: format!("long-running · {first_summary}"),
        duration_ms: (oldest_ms > 0).then_some(oldest_ms),
    }
}

fn recent_tool_rows(app: &App, limit: usize) -> Vec<SidebarToolRow> {
    let rows: Vec<SidebarToolRow> = app
        .history
        .iter()
        .rev()
        .filter_map(sidebar_tool_row_from_cell)
        .take(RECENT_TOOL_SCAN_LIMIT)
        .collect();
    editorial_tool_rows(rows, limit)
}

fn push_tool_rows(
    lines: &mut Vec<Line<'static>>,
    rows: &[SidebarToolRow],
    content_width: usize,
    max_rows: usize,
) {
    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let (marker, color) = tool_status_marker(row.status);
        let label = if let Some(duration_ms) = row.duration_ms {
            format!("{marker} {} {}", row.name, format_duration_ms(duration_ms))
        } else {
            format!("{marker} {}", row.name)
        };
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&label, content_width),
            Style::default().fg(color),
        )));
        if !row.summary.trim().is_empty() && lines.len() < max_rows {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_line_to_width(&row.summary, content_width.saturating_sub(2).max(1))
                ),
                Style::default().fg(palette::TEXT_DIM),
            )));
        }
    }
}

fn sidebar_tool_row_from_cell(cell: &HistoryCell) -> Option<SidebarToolRow> {
    let HistoryCell::Tool(tool) = cell else {
        return None;
    };
    match tool {
        ToolCell::Exec(exec) => Some(SidebarToolRow {
            name: concise_shell_command_label(&exec.command, 48),
            status: shell_status_for_sidebar(
                &exec.command,
                exec.status,
                exec.output_summary.as_deref(),
                exec.output.as_deref(),
            ),
            summary: shell_summary_for_sidebar(
                &exec.command,
                exec.status,
                exec.output_summary.as_deref(),
                exec.output.as_deref(),
            ),
            duration_ms: exec.duration_ms.or_else(|| {
                (exec.status == ToolStatus::Running).then(|| {
                    u64::try_from(
                        exec.started_at
                            .map(|started| started.elapsed().as_millis())
                            .unwrap_or_default(),
                    )
                    .unwrap_or(u64::MAX)
                })
            }),
        }),
        ToolCell::Exploring(explore) => {
            let running = explore
                .entries
                .iter()
                .filter(|entry| entry.status == ToolStatus::Running)
                .count();
            let status = if running > 0 {
                ToolStatus::Running
            } else if explore
                .entries
                .iter()
                .any(|entry| entry.status == ToolStatus::Failed)
            {
                ToolStatus::Failed
            } else {
                ToolStatus::Success
            };
            let first = explore.entries.first().map(|entry| entry.label.as_str());
            Some(SidebarToolRow {
                name: "workspace".to_string(),
                status,
                summary: compact_join([
                    format!("{} item(s), {running} running", explore.entries.len()),
                    first.unwrap_or_default().to_string(),
                ]),
                duration_ms: None,
            })
        }
        ToolCell::PlanUpdate(plan) => Some(SidebarToolRow {
            name: "update_plan".to_string(),
            status: plan.status,
            summary: plan
                .explanation
                .as_deref()
                .or_else(|| plan.steps.first().map(|step| step.step.as_str()))
                .unwrap_or("")
                .to_string(),
            duration_ms: None,
        }),
        ToolCell::PatchSummary(patch) => Some(SidebarToolRow {
            name: "patch".to_string(),
            status: patch.status,
            summary: compact_join([patch.path.clone(), patch.summary.clone()]),
            duration_ms: None,
        }),
        ToolCell::Review(review) => Some(SidebarToolRow {
            name: "review".to_string(),
            status: review.status,
            summary: review.target.clone(),
            duration_ms: None,
        }),
        ToolCell::DiffPreview(diff) => Some(SidebarToolRow {
            name: "diff".to_string(),
            status: ToolStatus::Success,
            summary: diff.title.clone(),
            duration_ms: None,
        }),
        ToolCell::Mcp(mcp) => Some(SidebarToolRow {
            name: mcp.tool.clone(),
            status: mcp.status,
            summary: mcp
                .content
                .as_deref()
                .map(summarize_tool_output)
                .unwrap_or_default(),
            duration_ms: None,
        }),
        ToolCell::ViewImage(image) => Some(SidebarToolRow {
            name: "image".to_string(),
            status: ToolStatus::Success,
            summary: image.path.display().to_string(),
            duration_ms: None,
        }),
        ToolCell::WebSearch(search) => Some(SidebarToolRow {
            name: "web_search".to_string(),
            status: search.status,
            summary: compact_join([
                search.query.clone(),
                search.summary.clone().unwrap_or_default(),
            ]),
            duration_ms: None,
        }),
        ToolCell::Generic(generic) => Some(SidebarToolRow {
            name: friendly_generic_tool_name(&generic.name).to_string(),
            status: generic.status,
            summary: generic_tool_sidebar_summary(generic),
            duration_ms: None,
        }),
    }
}

fn shell_status_for_sidebar(
    command: &str,
    status: ToolStatus,
    output_summary: Option<&str>,
    output: Option<&str>,
) -> ToolStatus {
    if status == ToolStatus::Failed && looks_like_pending_ci(command, output_summary, output) {
        ToolStatus::Running
    } else {
        status
    }
}

fn shell_summary_for_sidebar(
    command: &str,
    status: ToolStatus,
    output_summary: Option<&str>,
    output: Option<&str>,
) -> String {
    if status == ToolStatus::Failed && looks_like_pending_ci(command, output_summary, output) {
        return format!(
            "Waiting for CI \u{00B7} {} details",
            crate::tui::key_shortcuts::tool_details_shortcut_label()
        );
    }

    let summary = compact_join([
        output_summary.unwrap_or_default().to_string(),
        output
            .map(first_nonempty_line)
            .unwrap_or_default()
            .to_string(),
    ]);
    if status == ToolStatus::Failed {
        failure_summary_with_hint(&summary)
    } else {
        summary
    }
}

fn looks_like_pending_ci(
    command: &str,
    output_summary: Option<&str>,
    output: Option<&str>,
) -> bool {
    let command_label = concise_shell_command_label(command, 80).to_ascii_lowercase();
    if !command_label.starts_with("gh pr checks") && !command_label.starts_with("gh run watch") {
        return false;
    }

    let text = compact_join([
        output_summary.unwrap_or_default().to_string(),
        output.unwrap_or_default().to_string(),
    ])
    .to_ascii_lowercase();
    if text.is_empty() {
        return false;
    }
    let pending = ["pending", "queued", "in_progress", "in progress", "waiting"]
        .iter()
        .any(|needle| text.contains(needle));
    let hard_failure = ["failed", "failure", "error", "cancelled", "canceled"]
        .iter()
        .any(|needle| text.contains(needle));
    pending && !hard_failure
}

fn failure_summary_with_hint(summary: &str) -> String {
    let hint = format!(
        "inspect details with {}",
        crate::tui::key_shortcuts::tool_details_shortcut_label()
    );
    if summary.trim().is_empty() {
        hint
    } else if summary.contains(&hint) {
        summary.to_string()
    } else {
        format!("{summary} \u{00B7} {hint}")
    }
}

fn friendly_generic_tool_name(name: &str) -> &str {
    match name {
        "task_shell_start" => "start shell job",
        "task_shell_wait" => "wait shell job",
        "task_shell_write" => "write shell job",
        _ => name,
    }
}

fn generic_tool_sidebar_summary(generic: &GenericToolCell) -> String {
    match generic.name.as_str() {
        "task_shell_start" => compact_join([
            generic.input_summary.clone().unwrap_or_default(),
            "background shell job".to_string(),
        ]),
        "task_shell_wait" => compact_join([
            generic.input_summary.clone().unwrap_or_default(),
            generic.output_summary.clone().unwrap_or_default(),
        ]),
        _ => compact_join([
            generic.input_summary.clone().unwrap_or_default(),
            generic.output_summary.clone().unwrap_or_default(),
            generic
                .output
                .as_deref()
                .map(summarize_tool_output)
                .unwrap_or_default(),
        ]),
    }
}

fn background_task_rows(app: &App, active_rows: &[SidebarToolRow]) -> Vec<TaskPanelEntry> {
    let mut rows: Vec<TaskPanelEntry> = app
        .task_panel
        .iter()
        .filter(|task| !background_task_duplicates_live_tool(task, active_rows))
        .cloned()
        .collect();
    rows.sort_by_key(|task| (task_status_rank(task.status.as_str()), task.id.clone()));
    rows
}

fn background_task_duplicates_live_tool(
    task: &TaskPanelEntry,
    active_rows: &[SidebarToolRow],
) -> bool {
    if task.status != "running" {
        return false;
    }

    if task.id.starts_with("rlm-") || task.prompt_summary.starts_with("RLM: ") {
        return active_rows
            .iter()
            .any(|row| row.status == ToolStatus::Running && row.name.starts_with("rlm_"));
    }

    let Some(command) = task.prompt_summary.strip_prefix("shell: ") else {
        return false;
    };
    let command = normalize_activity_text(command);
    !command.is_empty()
        && active_rows.iter().any(|row| {
            row.status == ToolStatus::Running
                && normalize_activity_text(&format!("{} {}", row.name, row.summary))
                    .contains(&command)
        })
}

fn editorial_tool_rows(rows: Vec<SidebarToolRow>, limit: usize) -> Vec<SidebarToolRow> {
    #[derive(Clone)]
    struct Candidate {
        rank: u8,
        order: usize,
        row: SidebarToolRow,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut low_value_groups: Vec<(usize, SidebarToolRow, usize)> = Vec::new();
    let mut ci_poll_groups: Vec<(usize, SidebarToolRow, usize)> = Vec::new();
    let mut seen_success: Vec<String> = Vec::new();

    for (order, mut row) in rows.into_iter().enumerate() {
        if row.status == ToolStatus::Failed {
            row.summary = failure_summary_with_hint(&row.summary);
        }

        if is_ci_poll_row(&row) {
            if let Some((_, grouped, count)) = ci_poll_groups
                .iter_mut()
                .find(|(_, grouped, _)| grouped.name == row.name)
            {
                *count += 1;
                if grouped.duration_ms.is_none() {
                    grouped.duration_ms = row.duration_ms;
                }
            } else {
                ci_poll_groups.push((order, row, 1));
            }
            continue;
        }

        if is_low_value_tool(&row.name) && row.status == ToolStatus::Success {
            if let Some((_, grouped, count)) = low_value_groups
                .iter_mut()
                .find(|(_, grouped, _)| grouped.name == row.name)
            {
                *count += 1;
                if grouped.summary.trim().is_empty() && !row.summary.trim().is_empty() {
                    grouped.summary = row.summary;
                }
            } else {
                low_value_groups.push((order, row, 1));
            }
            continue;
        }

        let key = sidebar_row_identity(&row);
        if row.status == ToolStatus::Success && seen_success.iter().any(|seen| seen == &key) {
            continue;
        }
        if row.status == ToolStatus::Success {
            seen_success.push(key);
        }

        candidates.push(Candidate {
            rank: tool_row_rank(&row),
            order,
            row,
        });
    }

    for (order, mut row, count) in ci_poll_groups {
        if count > 1 {
            let command = row.name.clone();
            row.name = "Waiting for CI".to_string();
            row.summary = format!(
                "{command} \u{00B7} {count} polls collapsed \u{00B7} {} details",
                crate::tui::key_shortcuts::tool_details_shortcut_label()
            );
            row.status = ToolStatus::Running;
        }
        candidates.push(Candidate {
            rank: tool_row_rank(&row),
            order,
            row,
        });
    }

    for (order, mut row, count) in low_value_groups {
        if count > 1 {
            row.name = format!("{} x{count}", row.name);
            if !row.summary.trim().is_empty() {
                row.summary = format!("latest: {}", row.summary);
            }
        }
        candidates.push(Candidate {
            rank: tool_row_rank(&row).saturating_add(1),
            order,
            row,
        });
    }

    candidates.sort_by_key(|candidate| (candidate.rank, candidate.order));
    candidates
        .into_iter()
        .take(limit)
        .map(|candidate| candidate.row)
        .collect()
}

fn sidebar_row_identity(row: &SidebarToolRow) -> String {
    format!(
        "{}\n{}",
        row.name.trim(),
        normalize_activity_text(row.summary.as_str())
    )
}

fn is_ci_poll_row(row: &SidebarToolRow) -> bool {
    row.name.starts_with("gh pr checks") || row.name.starts_with("gh run watch")
}

fn normalize_activity_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_row_rank(row: &SidebarToolRow) -> u8 {
    match row.status {
        ToolStatus::Failed => 0,
        ToolStatus::Running => 1,
        ToolStatus::Success if is_low_value_tool(&row.name) => 3,
        ToolStatus::Success => 2,
    }
}

fn task_status_rank(status: &str) -> u8 {
    match status {
        "running" => 0,
        "failed" => 1,
        "queued" => 2,
        "completed" => 3,
        "canceled" => 4,
        _ => 5,
    }
}

fn is_low_value_tool(name: &str) -> bool {
    let base = name.split_whitespace().next().unwrap_or(name);
    matches!(
        base,
        "read_file" | "grep_files" | "file_search" | "find" | "checklist_update"
    )
}

fn compact_join(parts: impl IntoIterator<Item = String>) -> String {
    let mut out: Vec<String> = Vec::new();
    for part in parts {
        let part = part.trim();
        if !part.is_empty() && !out.iter().any(|seen| seen == part) {
            out.push(part.to_string());
        }
    }
    out.join(" · ")
}

fn first_nonempty_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
}

fn tool_status_marker(status: ToolStatus) -> (&'static str, ratatui::style::Color) {
    match status {
        ToolStatus::Running => ("[~]", palette::STATUS_WARNING),
        ToolStatus::Success => ("[x]", palette::STATUS_SUCCESS),
        ToolStatus::Failed => ("[!]", palette::STATUS_ERROR),
    }
}

fn format_duration_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1000.0)
    }
}

fn duration_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

fn render_sidebar_subagents(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let usable_rows = area.height.saturating_sub(3) as usize;
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
        .filter(|agent| matches!(agent.status, SubAgentStatus::Running))
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
    let foreground_rlm_running = foreground_rlm_running(app);

    let summary = SidebarSubagentSummary {
        cached_total: app.subagent_cache.len(),
        cached_running,
        progress_only_count,
        fanout_total,
        fanout_running,
        foreground_rlm_running,
        role_counts,
    };
    let rows = sidebar_agent_rows(app);
    let lines = subagent_panel_lines(&summary, &rows, content_width, usable_rows.max(1));

    render_sidebar_section(f, area, "Agents", lines, app);
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

#[derive(Debug, Clone)]
pub struct SidebarAgentRow {
    pub id: String,
    pub name: String,
    pub role: String,
    pub status: String,
    pub progress: Option<String>,
    pub steps_taken: u32,
    pub duration_ms: Option<u64>,
}

fn foreground_rlm_running(app: &App) -> bool {
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

fn sidebar_agent_rows(app: &App) -> Vec<SidebarAgentRow> {
    let mut rows: Vec<SidebarAgentRow> = app
        .subagent_cache
        .iter()
        .map(|agent| {
            let progress = app
                .agent_progress
                .get(&agent.agent_id)
                .cloned()
                .or_else(|| {
                    agent
                        .result
                        .as_deref()
                        .map(summarize_tool_output)
                        .filter(|summary| !summary.trim().is_empty())
                });
            SidebarAgentRow {
                id: agent.agent_id.clone(),
                name: agent.nickname.clone().unwrap_or_else(|| agent.name.clone()),
                role: agent.agent_type.as_str().to_string(),
                status: subagent_status_text(&agent.status).to_string(),
                progress,
                steps_taken: agent.steps_taken,
                duration_ms: Some(agent.duration_ms),
            }
        })
        .collect();

    let cached_ids: std::collections::HashSet<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    rows.extend(
        app.agent_progress
            .iter()
            .filter(|(id, _)| !cached_ids.contains(id.as_str()))
            .map(|(id, progress)| SidebarAgentRow {
                id: id.clone(),
                name: id.clone(),
                role: "agent".to_string(),
                status: "running".to_string(),
                progress: Some(progress.clone()),
                steps_taken: 0,
                duration_ms: app.agent_activity_started_at.map(|started| {
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
                }),
            }),
    );

    rows
}

fn subagent_status_text(status: &SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "running",
        SubAgentStatus::Completed => "done",
        SubAgentStatus::Interrupted(_) => "interrupted",
        SubAgentStatus::Failed(_) => "failed",
        SubAgentStatus::Cancelled => "canceled",
    }
}

/// Build sub-agent sidebar lines from summary + per-agent rows. Public
/// for the snapshot tests in this module.
pub fn subagent_panel_lines(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    content_width: usize,
    max_rows: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    let fanout_total = summary.fanout_total.unwrap_or(0);
    if summary.cached_total == 0
        && summary.progress_only_count == 0
        && fanout_total == 0
        && !summary.foreground_rlm_running
    {
        lines.push(Line::from(Span::styled(
            "No agents",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        return lines;
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
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            ),
            Span::styled(
                format!(" / {total}"),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]
    } else {
        vec![Span::styled(
            format!("{done} done"),
            Style::default().fg(palette::STATUS_SUCCESS),
        )]
    };
    lines.push(Line::from(header));

    if !summary.role_counts.is_empty() {
        let mix: Vec<String> = summary
            .role_counts
            .iter()
            .map(|(role, count)| format!("{count} {role}"))
            .collect();
        let role_line = mix.join(" \u{00B7} ");
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&role_line, content_width.max(1)),
            Style::default().fg(palette::TEXT_DIM),
        )));
    }

    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let (marker, color) = agent_status_marker(row.status.as_str());
        let label = format!("{marker} {} {}", row.role, row.name);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&label, content_width.max(1)),
            Style::default().fg(color),
        )));

        if lines.len() >= max_rows {
            break;
        }
        let mut detail_parts = Vec::new();
        detail_parts.push(truncate_line_to_width(&row.id, 10));
        if row.steps_taken > 0 {
            detail_parts.push(format!("{} step(s)", row.steps_taken));
        }
        if let Some(duration) = row.duration_ms {
            detail_parts.push(format_duration_ms(duration));
        }
        if let Some(progress) = row.progress.as_deref()
            && !progress.trim().is_empty()
        {
            detail_parts.push(summarize_tool_output(progress));
        }
        lines.push(Line::from(Span::styled(
            format!(
                "  {}",
                truncate_line_to_width(
                    &detail_parts.join(" · "),
                    content_width.saturating_sub(2).max(1)
                )
            ),
            Style::default().fg(palette::TEXT_DIM),
        )));
    }

    if summary.foreground_rlm_running {
        lines.push(Line::from(vec![
            Span::styled("RLM", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::styled(
                " foreground work active",
                Style::default().fg(palette::TEXT_DIM),
            ),
        ]));
    }

    lines
}

fn agent_status_marker(status: &str) -> (&'static str, ratatui::style::Color) {
    match status {
        "running" => ("[~]", palette::STATUS_WARNING),
        "done" => ("[x]", palette::STATUS_SUCCESS),
        "failed" => ("[!]", palette::STATUS_ERROR),
        "canceled" | "interrupted" => ("[-]", palette::TEXT_MUTED),
        _ => ("[ ]", palette::TEXT_MUTED),
    }
}

/// Session-context panel (#504) — consolidated session state overview.
///
/// Surfaces at-a-glance: working set, token usage / context %, running
/// cost, MCP server count, LSP toggle state, cycle count, and memory
/// file size + mtime. Each section is a compact one-liner so the panel
/// reads as a dashboard rather than a scrolling list.
fn render_context_panel(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(usize::from(area.height).max(4));

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
            Style::default().fg(palette::DEEPSEEK_SKY).bold(),
        ),
        Span::styled(
            format!("  {}", app.workspace_context.as_deref().unwrap_or("")),
            Style::default().fg(palette::TEXT_DIM),
        ),
    ]));

    // ── Token usage ──────────────────────────────────────────────
    let total_tokens = app.session.total_conversation_tokens;
    let window = crate::models::context_window_for_model(&app.model).unwrap_or(1_048_576);
    let pct = if window > 0 {
        ((total_tokens as f64 / window as f64) * 100.0).clamp(0.0, 100.0)
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
        format!(
            "context: {}/{} tokens  {}",
            total_tokens,
            window,
            truncate_line_to_width(&bar, content_width.saturating_sub(32).max(8))
        ),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    // ── Session cost ─────────────────────────────────────────────
    let displayed_total = app.displayed_session_cost_for_currency(app.cost_currency);
    let session_cost = app.session_cost_for_currency(app.cost_currency);
    let agent_cost = app.subagent_cost_for_currency(app.cost_currency);
    let real_total = session_cost + agent_cost;
    // Only show the additive breakdown when it matches the displayed
    // total; when the high-water mark is in effect (post-reconciliation),
    // the breakdown would not sum to the displayed value (#244).
    let cost_line = if (displayed_total - real_total).abs() < COST_EQ_TOLERANCE {
        format!(
            "cost: {} (session {} + agents {})",
            app.format_cost_amount(displayed_total),
            app.format_cost_amount(session_cost),
            app.format_cost_amount(agent_cost)
        )
    } else {
        format!("cost: {}", app.format_cost_amount(displayed_total))
    };
    lines.push(Line::from(Span::styled(
        cost_line,
        Style::default().fg(palette::TEXT_MUTED),
    )));

    // ── MCP servers ──────────────────────────────────────────────
    if app.mcp_configured_count > 0 {
        let restart_hint = if app.mcp_restart_required {
            " (restart needed)"
        } else {
            ""
        };
        lines.push(Line::from(Span::styled(
            format!(
                "mcp: {} server(s){}",
                app.mcp_configured_count, restart_hint
            ),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    // ── LSP ──────────────────────────────────────────────────────
    let lsp_label = if app.lsp_enabled { "on" } else { "off" };
    lines.push(Line::from(Span::styled(
        format!("lsp: {}", lsp_label),
        Style::default().fg(palette::TEXT_MUTED),
    )));

    // ── Cycles ───────────────────────────────────────────────────
    if app.cycle_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "cycles: {} crossed, {} briefing(s)",
                app.cycle_count,
                app.cycle_briefings.len()
            ),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    // ── Memory ───────────────────────────────────────────────────
    if app.use_memory {
        let size_hint = std::fs::metadata(&app.memory_path)
            .map(|m| m.len())
            .map(|bytes| {
                if bytes >= 1024 * 1024 {
                    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
                } else if bytes >= 1024 {
                    format!("{:.1} KB", bytes as f64 / 1024.0)
                } else {
                    format!("{} B", bytes)
                }
            })
            .unwrap_or_else(|_| "—".to_string());
        lines.push(Line::from(Span::styled(
            format!("memory: {} ({})", app.memory_path.display(), size_hint),
            Style::default().fg(palette::TEXT_MUTED),
        )));
    }

    render_sidebar_section(f, area, "Session", lines, app);
}

fn render_sidebar_section(
    f: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    app: &App,
) {
    if area.width < 4 || area.height < 3 {
        // Clear stale cells before bailing out (#400).
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(area, f.buffer_mut());
        return;
    }

    let theme = Theme::for_palette_mode(app.ui_theme.mode);
    // Truncate the panel title so it always fits within the section width
    // even after a resize. The title occupies up to 4 chars of border chrome
    // (two spaces + one space on each side), so the max title length is
    // area.width.saturating_sub(4) when borders are enabled.
    let max_title_width = area.width.saturating_sub(4).max(1) as usize;
    let display_title = truncate_line_to_width(title, max_title_width);

    // Constrain lines to the visible section area so a Paragraph wrap
    // overflow can't write cells outside the Block bounds (#400). The
    // border + padding consume 2 rows; budget the rest for content.
    let visible_content_rows = area
        .height
        .saturating_sub(2) // top + bottom border
        .saturating_sub(theme.section_padding.top + theme.section_padding.bottom)
        as usize;
    let lines: Vec<Line<'static>> =
        if lines.len() > visible_content_rows && visible_content_rows > 0 {
            lines.into_iter().take(visible_content_rows).collect()
        } else {
            lines
        };

    let section = Paragraph::new(lines).wrap(Wrap { trim: true }).block(
        Block::default()
            .title(Line::from(vec![Span::styled(
                format!(" {display_title} "),
                Style::default().fg(theme.section_title_color).bold(),
            )]))
            .borders(theme.section_borders)
            .border_type(theme.section_border_type)
            .border_style(Style::default().fg(theme.section_border_color))
            .style(Style::default().bg(theme.section_bg))
            .padding(theme.section_padding),
    );

    f.render_widget(section, area);
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_TOOL_COMPLETED_ROW_TTL, ACTIVE_TOOL_STALE_RUNNING_ROW_TTL, AutoSidebarPanel,
        AutoSidebarState, SidebarAgentRow, SidebarSubagentSummary, SidebarWorkChecklistItem,
        SidebarWorkStrategyStep, SidebarWorkSummary, auto_sidebar_panels, subagent_panel_lines,
        task_panel_lines, work_panel_empty_hint, work_panel_lines,
    };
    use crate::config::Config;
    use crate::palette::PaletteMode;
    use crate::tools::plan::StepStatus;
    use crate::tools::todo::TodoStatus;
    use crate::tui::active_cell::ActiveCell;
    use crate::tui::app::{App, TaskPanelEntry, TuiOptions};
    use crate::tui::history::{
        ExecCell, ExecSource, GenericToolCell, HistoryCell, ToolCell, ToolStatus,
    };
    use ratatui::text::Line;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn create_test_app() -> App {
        let options = TuiOptions {
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
        };
        App::new(options, &Config::default())
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
    fn auto_sidebar_does_not_reserve_empty_work_when_other_panels_are_active() {
        let panels = auto_sidebar_panels(AutoSidebarState {
            work_has_content: false,
            tasks_empty: false,
            agents_empty: true,
            context_enabled: false,
        });

        assert_eq!(panels, vec![AutoSidebarPanel::Tasks]);
    }

    #[test]
    fn auto_sidebar_uses_work_as_single_empty_state() {
        let panels = auto_sidebar_panels(AutoSidebarState {
            work_has_content: false,
            tasks_empty: true,
            agents_empty: true,
            context_enabled: false,
        });

        assert_eq!(panels, vec![AutoSidebarPanel::Work]);
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
    fn work_panel_renders_checklist_as_primary_progress_surface() {
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
            strategy_explanation: Some("Keep the UI unified".to_string()),
            strategy_steps: vec![
                SidebarWorkStrategyStep {
                    text: "Simplify sidebar".to_string(),
                    status: StepStatus::Completed,
                    elapsed: String::new(),
                },
                SidebarWorkStrategyStep {
                    text: "Update prompts".to_string(),
                    status: StepStatus::Pending,
                    elapsed: String::new(),
                },
            ],
            ..SidebarWorkSummary::default()
        };

        let text = lines_to_text(&work_panel_lines(&summary, 80, 16, PaletteMode::Dark));

        assert!(
            text[0].starts_with("33% complete (1/3)"),
            "checklist should lead: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("[~] #2 Wire")),
            "in-progress checklist item should be visible: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("50% complete")),
            "strategy progress must not render as a second progress bar when checklist exists: {text:?}"
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

        let text = lines_to_text(&work_panel_lines(&summary, 80, 6, PaletteMode::Dark));

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
    fn work_panel_includes_strategy_only_when_plan_state_is_non_empty() {
        let empty_text = lines_to_text(&work_panel_lines(
            &SidebarWorkSummary::default(),
            80,
            16,
            PaletteMode::Dark,
        ));
        assert!(
            !empty_text.iter().any(|line| line.contains("Strategy")),
            "empty plan state should not show strategy: {empty_text:?}"
        );

        let summary = SidebarWorkSummary {
            strategy_explanation: Some("High-level sequencing".to_string()),
            ..SidebarWorkSummary::default()
        };
        let text = lines_to_text(&work_panel_lines(&summary, 80, 16, PaletteMode::Dark));
        assert!(
            text.iter().any(|line| line == "Strategy"),
            "non-empty plan should show strategy label: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("High-level sequencing")),
            "non-empty plan explanation should render: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_renders_active_tool_rows_before_background_empty_state() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "agent_eval".to_string(),
                status: ToolStatus::Running,
                input_summary: Some("agent_id: agent_af58ba3a".to_string()),
                output: None,
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);
        app.runtime_turn_id = Some("turn_abcdef123456".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));

        assert!(text[0].contains("turn "));
        assert!(text[0].contains("in_progress"));
        assert!(
            text.iter().any(|line| line == "Live tools"),
            "live section missing: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("[~] agent_eval")),
            "active agent_eval row missing: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("No active tasks")),
            "old empty state should not render during active tools: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_renders_recent_completed_tool_rows() {
        let mut app = create_test_app();
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "read_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("deepseek-tui/CHANGELOG.md".to_string()),
                output: Some("done".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: Some("Reading CHANGELOG.md".to_string()),
                is_diff: false,
            })));

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));

        assert!(
            text.iter().any(|line| line == "Recent tools"),
            "recent section missing: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("[x] read_file")),
            "recent read_file row missing: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_expires_completed_active_tool_rows() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "read_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("src/main.rs".to_string()),
                output: Some("done".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: Some("done".to_string()),
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);
        app.active_tool_entry_completed_at.insert(
            0,
            Instant::now() - ACTIVE_TOOL_COMPLETED_ROW_TTL - Duration::from_secs(1),
        );

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));

        assert!(
            !text.iter().any(|line| line.contains("[x] read_file")),
            "expired completed active row should leave the sidebar: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_lingers_fresh_completed_active_tool_rows() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "read_file".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("src/main.rs".to_string()),
                output: Some("done".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: Some("done".to_string()),
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);
        app.active_tool_entry_completed_at.insert(0, Instant::now());

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));

        assert!(
            text.iter().any(|line| line.contains("[x] read_file")),
            "fresh completed active row should linger briefly: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_collapses_stale_running_tool_rows() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        for (idx, command) in ["long one", "long two"].into_iter().enumerate() {
            active.push_tool(
                format!("shell-{idx}"),
                HistoryCell::Tool(ToolCell::Exec(ExecCell {
                    command: command.to_string(),
                    status: ToolStatus::Running,
                    output: None,
                    started_at: None,
                    duration_ms: Some(ACTIVE_TOOL_STALE_RUNNING_ROW_TTL.as_millis() as u64 + 1),
                    source: ExecSource::Assistant,
                    interaction: None,
                    output_summary: None,
                })),
            );
        }
        app.active_cell = Some(active);

        let text = lines_to_text(&task_panel_lines(&app, 80, 8));

        assert!(
            text.iter().any(|line| line.contains("[~] run x2")),
            "stale running rows should collapse into one sidebar row: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("long two")),
            "second stale command should not take another row: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_does_not_double_count_running_shell_job_as_live_and_background() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "shell-1",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "cargo test --workspace".to_string(),
                status: ToolStatus::Running,
                output: None,
                started_at: Some(std::time::Instant::now()),
                duration_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            })),
        );
        app.active_cell = Some(active);
        app.task_panel.push(TaskPanelEntry {
            id: "job_123".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test --workspace".to_string(),
            duration_ms: Some(12_000),
        });

        let text = lines_to_text(&task_panel_lines(&app, 80, 10));
        let command_lines = text
            .iter()
            .filter(|line| line.contains("cargo test --workspace"))
            .count();

        assert!(
            text.iter().any(|line| line == "Live tools"),
            "live shell row missing: {text:?}"
        );
        assert_eq!(
            command_lines, 1,
            "running shell command should not render as both live and background: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("Background jobs")),
            "duplicate background shell row should be hidden: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_collapses_repeated_low_value_recent_tools_after_failures() {
        let mut app = create_test_app();
        for path in ["src/a.rs", "src/b.rs", "src/c.rs"] {
            app.history
                .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                    name: "read_file".to_string(),
                    status: ToolStatus::Success,
                    input_summary: Some(path.to_string()),
                    output: Some("ok".to_string()),
                    prompts: None,
                    spillover_path: None,
                    output_summary: None,
                    is_diff: false,
                })));
        }
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "checklist_update".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("mark item 2 done".to_string()),
                output: Some("updated".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })));
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "grep_files".to_string(),
                status: ToolStatus::Failed,
                input_summary: Some("pattern: Activity Detail".to_string()),
                output: Some("regex parse error".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: Some("regex parse error".to_string()),
                is_diff: false,
            })));

        let text = lines_to_text(&task_panel_lines(&app, 80, 12));
        let failed_index = text
            .iter()
            .position(|line| line.contains("[!] grep_files"))
            .expect("failed grep row should stay visible");
        let read_group_index = text
            .iter()
            .position(|line| line.contains("[x] read_file x3"))
            .expect("repeated read_file rows should collapse");

        assert!(
            failed_index < read_group_index,
            "failure should sort above low-value success noise: {text:?}"
        );
        assert_eq!(
            text.iter()
                .filter(|line| line.contains("[x] read_file"))
                .count(),
            1,
            "read_file should render once after grouping: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("regex parse error")),
            "failure detail should remain visible: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_collapses_repeated_pending_ci_polls() {
        let mut app = create_test_app();
        for _ in 0..3 {
            app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "cd /tmp/repo && sleep 15 && gh pr checks 1616 --repo Hmbown/DeepSeek-TUI"
                    .to_string(),
                status: ToolStatus::Failed,
                output: Some("Lint pending\nTest pending".to_string()),
                started_at: None,
                duration_ms: Some(15_000),
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: Some("2 checks pending".to_string()),
            })));
        }

        let text = lines_to_text(&task_panel_lines(&app, 80, 12));

        assert!(
            text.iter().any(|line| line.contains("[~] Waiting for CI")),
            "pending CI should not render as a hard failure: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("gh pr checks 1616")),
            "concise command label should remain visible: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("3 polls collapsed")),
            "repeated polling should collapse into one row: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains(crate::tui::key_shortcuts::tool_details_shortcut_label())),
            "collapsed CI row should point to details: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("[!] gh pr checks")),
            "pending CI should not look like a real failure: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_failed_shell_rows_point_to_activity_details() {
        let mut app = create_test_app();
        app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test -p deepseek-tui".to_string(),
            status: ToolStatus::Failed,
            output: Some("test failed".to_string()),
            started_at: None,
            duration_ms: Some(1_250),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: Some("test failed".to_string()),
        })));

        let text = lines_to_text(&task_panel_lines(&app, 80, 8));

        assert!(
            text.iter().any(|line| line.contains("[!] cargo test")),
            "failed shell command should keep its concise label: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains(&format!(
                "inspect details with {}",
                crate::tui::key_shortcuts::tool_details_shortcut_label()
            ))),
            "failed row should include the next action: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_keeps_duration_and_status_on_recent_shell_rows() {
        let mut app = create_test_app();
        app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo check".to_string(),
            status: ToolStatus::Success,
            output: Some("Finished".to_string()),
            started_at: None,
            duration_ms: Some(1_250),
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        })));

        let text = lines_to_text(&task_panel_lines(&app, 80, 8));

        assert!(
            text.iter()
                .any(|line| line.contains("[x] cargo check 1.2s")),
            "status marker and duration should stay in the row label: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("cargo check")),
            "current command summary should stay visible: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_uses_plain_names_for_shell_background_helpers() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "shell-wait",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "task_shell_wait".to_string(),
                status: ToolStatus::Running,
                input_summary: Some("task_id: shell_33a08c3c".to_string()),
                output: None,
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);

        let text = lines_to_text(&task_panel_lines(&app, 80, 6));

        assert!(
            text.iter().any(|line| line.contains("[~] wait shell job")),
            "shell helper should render as a user-facing activity: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("task_shell_wait")),
            "internal helper name should not leak into sidebar: {text:?}"
        );
    }

    #[test]
    fn navigator_empty_state_says_no_agents() {
        let summary = SidebarSubagentSummary::default();
        let lines = subagent_panel_lines(&summary, &[], 32, 8);
        let text = lines_to_text(&lines);
        assert_eq!(text, vec!["No agents".to_string()]);
    }

    #[test]
    fn agents_panel_running_state_renders_count_role_and_rows() {
        // Two general agents (one running, one done) + one explore (running).
        let mut role_counts = std::collections::BTreeMap::new();
        role_counts.insert("general".to_string(), 2);
        role_counts.insert("explore".to_string(), 1);
        let summary = SidebarSubagentSummary {
            cached_total: 3,
            cached_running: 2,
            progress_only_count: 0,
            fanout_total: None,
            fanout_running: 0,
            foreground_rlm_running: false,
            role_counts,
        };
        let rows = vec![
            SidebarAgentRow {
                id: "agent_a5e674dc".to_string(),
                name: "check-docs-mcp".to_string(),
                role: "explore".to_string(),
                status: "running".to_string(),
                progress: Some("step 2/3: running tool 'read_file'".to_string()),
                steps_taken: 2,
                duration_ms: Some(22_000),
            },
            SidebarAgentRow {
                id: "agent_850aa63f".to_string(),
                name: "check-install-docs".to_string(),
                role: "general".to_string(),
                status: "done".to_string(),
                progress: Some("SUMMARY: docs checked".to_string()),
                steps_taken: 5,
                duration_ms: Some(21_000),
            },
        ];
        let text = lines_to_text(&subagent_panel_lines(&summary, &rows, 64, 12));
        assert!(text[0].contains("2 running"), "header: {:?}", text[0]);
        assert!(text[0].contains("/ 3"), "total in header: {:?}", text[0]);
        assert!(
            text[1].contains("1 explore") && text[1].contains("2 general"),
            "role mix line: {:?}",
            text[1]
        );
        assert!(
            text.iter()
                .any(|l| l.contains("[~] explore check-docs-mcp")),
            "running row missing: {text:?}",
        );
        assert!(
            text.iter().any(|l| l.contains("step 2/3")),
            "progress detail missing: {text:?}",
        );
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

        let text = lines_to_text(&subagent_panel_lines(&summary, &[], 64, 8));

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
        let text = lines_to_text(&subagent_panel_lines(&summary, &[], 32, 8));
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
        let lines = subagent_panel_lines(&summary, &[], 16, 8);
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
        let text = lines_to_text(&subagent_panel_lines(&summary, &[], 64, 8));

        assert!(!text[0].contains("No agents"), "header: {:?}", text);
        assert!(
            text.iter()
                .any(|line| line.contains("RLM foreground work active")),
            "RLM work must be visible in Agents panel: {text:?}"
        );
    }
}
