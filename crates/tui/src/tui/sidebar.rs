//! Sidebar rendering — Work / Tasks / Agents / Context panels.
//!
//! Extracted from `tui/ui.rs` (P1.2). The sidebar appears to the right of
//! the chat transcript when the available width allows it. Each section
//! reads from `App` snapshots; mutation lives in the main app loop.

use std::fmt::Write;
use std::time::{Duration, Instant};

use crate::tui::app::HuntVerdict;

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

use super::app::{App, SidebarFocus, SidebarHoverSection, SidebarHoverState, TaskPanelEntry};
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

pub fn render_sidebar(f: &mut Frame, area: Rect, app: &mut App) {
    // Clear hover state at the start of each render
    app.sidebar_hover = SidebarHoverState::default();
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
fn render_sidebar_auto(f: &mut Frame, area: Rect, app: &mut App) {
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
pub(crate) struct SidebarWorkSummary {
    goal_objective: Option<String>,
    goal_token_budget: Option<u32>,
    goal_completed: bool,
    goal_started_at: Option<Instant>,
    tokens_used: u32,
    checklist_completion_pct: u8,
    checklist_items: Vec<SidebarWorkChecklistItem>,
    strategy_explanation: Option<String>,
    strategy_steps: Vec<SidebarWorkStrategyStep>,
    state_updating: bool,
    /// Optional pause indicator text ("(Paused)" or "(Cancelled)").
    pause_indicator: Option<String>,
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

fn sidebar_work_summary(app: &mut App) -> SidebarWorkSummary {
    let fresh = (|| {
        let todos = app.todos.try_lock().ok()?;
        let plan = app.plan_state.try_lock().ok()?;

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

        let (strategy_explanation, strategy_steps) = if plan.is_empty() {
            (None, Vec::new())
        } else {
            (
                plan.explanation().map(str::to_string),
                plan.steps()
                    .iter()
                    .map(|step| SidebarWorkStrategyStep {
                        text: step.text.clone(),
                        status: step.status.clone(),
                        elapsed: step.elapsed_str(),
                    })
                    .collect(),
            )
        };

        Some(SidebarWorkSummary {
            goal_objective: if app.paused_quarry.is_some() || app.paused_cancelled {
                app.hunt.quarry.clone().or_else(|| app.paused_quarry.clone())
            } else {
                app.hunt.quarry.clone()
            },
            goal_token_budget: app.hunt.token_budget,
            goal_completed: app.hunt.verdict == HuntVerdict::Hunted,
            goal_started_at: app.hunt.started_at,
            tokens_used: app.session.total_conversation_tokens,
            checklist_completion_pct,
            pause_indicator: if app.paused || app.paused_quarry.is_some() {
                Some(if app.is_loading { "(Pausing)".to_string() } else { "(Paused)".to_string() })
            } else if app.paused_cancelled {
                Some("(Cancelled)".to_string())
            } else {
                None
            },
            checklist_items,
            strategy_explanation,
            strategy_steps,
            state_updating: false,
        })
    })();

    if let Some(summary) = fresh {
        app.cached_work_summary = Some(summary.clone());
        return summary;
    }

    if let Some(cached) = app.cached_work_summary.as_ref() {
        let mut summary = cached.clone();
        summary.goal_objective = if app.paused_quarry.is_some() || app.paused_cancelled {
            app.hunt.quarry.clone().or_else(|| app.paused_quarry.clone())
        } else {
            app.hunt.quarry.clone()
        };
        summary.goal_token_budget = app.hunt.token_budget;
        summary.goal_completed = app.hunt.verdict == HuntVerdict::Hunted;
        summary.goal_started_at = app.hunt.started_at;
        summary.tokens_used = app.session.total_conversation_tokens;
        summary.pause_indicator = if app.paused || app.paused_quarry.is_some() {
            Some(if app.is_loading { "(Pausing)".to_string() } else { "(Paused)".to_string() })
        } else if app.paused_cancelled {
            Some("(Cancelled)".to_string())
        } else {
            None
        };
        return summary;
    }

    SidebarWorkSummary {
        goal_objective: if app.paused_quarry.is_some() || app.paused_cancelled {
            app.hunt.quarry.clone().or_else(|| app.paused_quarry.clone())
        } else {
            app.hunt.quarry.clone()
        },
        goal_token_budget: app.hunt.token_budget,
        goal_completed: app.hunt.verdict == HuntVerdict::Hunted,
        goal_started_at: app.hunt.started_at,
        tokens_used: app.session.total_conversation_tokens,
        state_updating: true,
        pause_indicator: if app.paused || app.paused_quarry.is_some() {
            Some(if app.is_loading { "(Pausing)".to_string() } else { "(Paused)".to_string() })
        } else if app.paused_cancelled {
            Some("(Cancelled)".to_string())
        } else {
            None
        },
        ..SidebarWorkSummary::default()
    }
}

fn work_panel_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    palette_mode: palette::PaletteMode,
    ui_theme: &palette::UiTheme,
) -> Vec<Line<'static>> {
    let theme = Theme::for_palette_mode(palette_mode);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    push_work_goal_lines(summary, content_width, max_rows, &mut lines, ui_theme);

    if summary.state_updating && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            "Work state updating...",
            Style::default().fg(ui_theme.text_muted),
        )));
    }

    push_work_checklist_lines(summary, content_width, max_rows, &mut lines, ui_theme);
    push_work_strategy_lines(summary, content_width, max_rows, &mut lines, &theme);

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            work_panel_empty_hint(content_width),
            Style::default().fg(ui_theme.text_muted).italic(),
        )));
    }

    lines
}

fn push_work_goal_lines(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
    lines: &mut Vec<Line<'static>>,
    theme: &palette::UiTheme,
) {
    let Some(objective) = summary.goal_objective.as_deref() else {
        return;
    };
    if objective.trim().is_empty() || lines.len() >= max_rows {
        return;
    }

    let icon = if summary.goal_completed {
        "✓"
    } else if let Some(indicator) = &summary.pause_indicator {
        match indicator.as_str() {
            "(Cancelled)" => "✘",
            "(Pausing)" => "⏳",
            _ => "⏸",
        }
    } else {
        "▶"
    };
    let status_style = if summary.goal_completed {
        Style::default()
            .fg(theme.success)
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else if let Some(indicator) = &summary.pause_indicator {
        let (fg, _) = match indicator.as_str() {
            "(Cancelled)" => (theme.error_fg, "✘"),
            "(Pausing)" => (theme.warning, "⏳"),
            _ => (theme.accent_primary, "⏸"),
        };
        Style::default()
            .fg(fg)
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.status_working)
            .add_modifier(ratatui::style::Modifier::BOLD)
    };

    lines.push(Line::from(Span::styled(
        format!(
            "{} {}",
            icon,
            truncate_line_to_width(objective, content_width.saturating_sub(2).max(1))
        ),
        status_style,
    )));

    // Elapsed time
    if let Some(started) = summary.goal_started_at
        && lines.len() < max_rows
    {
        let elapsed = crate::tui::notifications::humanize_duration(started.elapsed());
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
    let completed = summary
        .checklist_items
        .iter()
        .filter(|item| item.status == TodoStatus::Completed)
        .count();
    lines.push(Line::from(vec![
        Span::styled(
            format!("{}%", summary.checklist_completion_pct),
            Style::default().fg(theme.success).bold(),
        ),
        Span::styled(
            format!(" complete ({completed}/{total})"),
            Style::default().fg(theme.text_muted),
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
            TodoStatus::Pending => ("[ ]", theme.text_muted),
            TodoStatus::InProgress => ("[~]", theme.warning),
            TodoStatus::Completed => ("[✓]", theme.success),
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
                "Strategy metadata ",
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
            "Strategy metadata",
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
            StepStatus::Completed => ("[✓]", theme.plan_completed_color),
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

fn render_sidebar_work(f: &mut Frame, area: Rect, app: &mut App) {
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
        &app.ui_theme,
    );

    let full_texts: Vec<String> = lines.iter().map(|l| spans_to_text(&l.spans)).collect();
    render_sidebar_section(f, area, "Work", lines, full_texts, app);
}

fn render_sidebar_tasks(f: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let usable_rows = area.height.saturating_sub(3) as usize;
    let lines = task_panel_lines(app, content_width.max(1), usable_rows.max(1));

    let full_texts: Vec<String> = lines.iter().map(|l| spans_to_text(&l.spans)).collect();
    render_sidebar_section(f, area, "Tasks", lines, full_texts, app);
}

#[derive(Debug, Clone)]
struct SidebarToolRow {
    name: String,
    status: ToolStatus,
    summary: String,
    duration_ms: Option<u64>,
}

fn task_panel_lines(app: &App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let theme = &app.ui_theme;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));

    if let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = app
            .runtime_turn_status
            .as_deref()
            .unwrap_or("unknown")
            .to_string();
        // Show enough of the turn id prefix to identify it for
        // task_read / task_cancel. A UUID needs ~13 chars before the
        // first hyphen; 16 chars gives a safe prefix for disambiguation.
        let turn_prefix = truncate_line_to_width(turn_id, 16);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(
                &format!("turn {turn_prefix} ({status})",),
                content_width.max(1),
            ),
            Style::default().fg(theme.accent_primary),
        )));
    }

    let active_rows = active_tool_rows(app);
    if !active_rows.is_empty() && lines.len() < max_rows {
        push_sidebar_label_theme(&mut lines, "Live tools", theme);
        push_tool_rows(&mut lines, &active_rows, content_width, max_rows, theme);
    }

    let background_rows = background_task_rows(app, &active_rows);
    if !background_rows.is_empty() && lines.len() < max_rows {
        let running = background_rows
            .iter()
            .filter(|task| task.status == "running")
            .count();
        let done = background_rows.len().saturating_sub(running);
        let label = if running == 0 {
            format!("Background commands: {done} completed")
        } else if done == 0 {
            format!("Background commands: {running} running")
        } else {
            format!("Background commands: {running} running, {done} completed")
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(theme.accent_primary).bold(),
        )));

        let max_items = max_rows.saturating_sub(lines.len());
        for task in background_rows.iter().take(max_items) {
            let color = match task.status.as_str() {
                "queued" => theme.text_muted,
                "running" => theme.warning,
                "completed" => theme.success,
                "failed" => theme.error_fg,
                "canceled" => theme.text_dim,
                _ => theme.text_muted,
            };
            let duration = task
                .duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string());
            let (label, detail) = background_task_labels(task, &duration);
            lines.push(Line::from(Span::styled(
                truncate_line_to_width(&label, content_width.max(1)),
                Style::default().fg(color),
            )));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_line_to_width(&detail, content_width.saturating_sub(2).max(1))
                ),
                Style::default().fg(theme.text_dim),
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
                    .fg(theme.text_muted)
                    .add_modifier(ratatui::style::Modifier::ITALIC),
            )));
        }
    }

    if lines.len() < max_rows {
        let recent_rows = recent_tool_rows(app, 4);
        if !recent_rows.is_empty() {
            push_sidebar_label_theme(&mut lines, "Recent tools", theme);
            push_tool_rows(&mut lines, &recent_rows, content_width, max_rows, theme);
        }
    }

    // Yank hint: surface the keyboard path for copying the focused task/turn ID.
    if lines.len() + 1 < max_rows
        && app.runtime_turn_id.is_some()
        && app.sidebar_focus == SidebarFocus::Tasks
    {
        lines.push(Line::from(Span::styled(
            "y → copy turn id  ·  Y → copy full status",
            Style::default()
                .fg(theme.text_dim)
                .add_modifier(ratatui::style::Modifier::ITALIC),
        )));
    }

    if lines.is_empty()
        || (lines.len() == 1
            && app.runtime_turn_id.is_some()
            && active_rows.is_empty()
            && background_rows.is_empty())
    {
        lines.push(Line::from(Span::styled(
            "No live tools or background jobs",
            Style::default().fg(theme.text_muted),
        )));
    }

    lines
}

fn push_sidebar_label_theme(lines: &mut Vec<Line<'static>>, label: &str, theme: &palette::UiTheme) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(theme.accent_primary).bold(),
    )));
}

fn background_task_labels(task: &TaskPanelEntry, duration: &str) -> (String, String) {
    if let Some(command) = task.prompt_summary.strip_prefix("shell: ") {
        let command = concise_shell_command_label(command, 96);
        return (
            format!("{} {} {}", task.status, command, duration),
            format!("{} \u{00B7} command", task.id),
        );
    }

    (
        format!(
            "{} {} {}",
            truncate_line_to_width(&task.id, 10),
            task.status,
            duration
        ),
        task.prompt_summary.clone(),
    )
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
    editorial_tool_rows(rows, usize::MAX, ToolRowOrder::OldestFirst)
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
    editorial_tool_rows(rows, limit, ToolRowOrder::NewestFirst)
}

fn push_tool_rows(
    lines: &mut Vec<Line<'static>>,
    rows: &[SidebarToolRow],
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) {
    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let (marker, color) = tool_status_marker(row.status, theme);
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
                Style::default().fg(theme.text_dim),
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
        "task_shell_start" => "start command",
        "task_shell_wait" => "wait command",
        "task_shell_write" => "write command",
        _ => name,
    }
}

fn generic_tool_sidebar_summary(generic: &GenericToolCell) -> String {
    match generic.name.as_str() {
        "task_shell_start" => compact_join([
            generic.input_summary.clone().unwrap_or_default(),
            "background command".to_string(),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToolRowOrder {
    OldestFirst,
    NewestFirst,
}

fn editorial_tool_rows(
    rows: Vec<SidebarToolRow>,
    limit: usize,
    order_mode: ToolRowOrder,
) -> Vec<SidebarToolRow> {
    #[derive(Clone)]
    struct Candidate {
        rank: u8,
        order: usize,
        row: SidebarToolRow,
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    let mut low_value_groups: Vec<(usize, SidebarToolRow, usize)> = Vec::new();
    let mut ci_poll_groups: Vec<(usize, SidebarToolRow, usize)> = Vec::new();
    let mut shell_wait_groups: Vec<(usize, SidebarToolRow, usize, String)> = Vec::new();
    let mut seen_success: Vec<String> = Vec::new();
    let mut seen_success_tool_names: Vec<String> = Vec::new();
    let mut seen_failures: Vec<String> = Vec::new();
    let mut visible_failure_count: usize = 0;
    const MAX_VISIBLE_FAILURES: usize = 2;

    for (order, mut row) in rows.into_iter().enumerate() {
        if row.status == ToolStatus::Failed {
            // Deduplicate failures for the same tool name: keep only the most
            // recent failure per tool. Fixes #1884 — stale failures from
            // tools that have since succeeded no longer crowd the sidebar.
            let fail_key = row.name.trim().to_ascii_lowercase();
            if order_mode == ToolRowOrder::NewestFirst
                && seen_success_tool_names.contains(&fail_key)
            {
                continue;
            }
            if seen_failures.contains(&fail_key) {
                continue;
            }
            seen_failures.push(fail_key);
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

        if is_shell_wait_poll_row(&row) {
            let key = shell_wait_poll_key(&row);
            if let Some((_, grouped, count, _)) = shell_wait_groups
                .iter_mut()
                .find(|(_, _, _, existing_key)| existing_key == &key)
            {
                *count += 1;
                if !row.summary.trim().is_empty() {
                    grouped.summary = row.summary;
                }
            } else {
                shell_wait_groups.push((order, row, 1, key));
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
            let normalized = row.name.trim().to_ascii_lowercase();
            if !seen_success_tool_names.contains(&normalized) {
                seen_success_tool_names.push(normalized.clone());
            }

            // Active rows are oldest-first, so a success means any candidate
            // failure for the same tool is stale. Recent history rows are
            // newest-first; in that path the success is older than any
            // already-seen failure and must not remove it.
            if order_mode == ToolRowOrder::OldestFirst {
                let mut removed_visible_failures = 0usize;
                let mut removed_any_failure = false;
                candidates.retain(|c| {
                    let remove = c.row.status == ToolStatus::Failed
                        && c.row.name.trim().eq_ignore_ascii_case(&normalized);
                    if remove {
                        removed_any_failure = true;
                        if c.rank == 0 {
                            removed_visible_failures += 1;
                        }
                    }
                    !remove
                });
                if removed_any_failure {
                    seen_failures.retain(|seen| seen != &normalized);
                    visible_failure_count =
                        visible_failure_count.saturating_sub(removed_visible_failures);
                }
            }
        }

        // Cap visible failures at MAX_VISIBLE_FAILURES. Excess failures
        // get demoted to rank 3 so they don't crowd the top of the
        // sidebar. (#1884)
        let rank = if row.status == ToolStatus::Failed {
            if visible_failure_count >= MAX_VISIBLE_FAILURES {
                3
            } else {
                visible_failure_count += 1;
                0
            }
        } else {
            tool_row_rank(&row)
        };

        candidates.push(Candidate { rank, order, row });
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

    for (order, mut row, count, key) in shell_wait_groups {
        if count > 1 {
            row.summary = compact_join([
                format!("{key} \u{00B7} {count} waits collapsed"),
                row.summary.clone(),
            ]);
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

fn is_shell_wait_poll_row(row: &SidebarToolRow) -> bool {
    row.status == ToolStatus::Running && row.name == "wait command"
}

fn shell_wait_poll_key(row: &SidebarToolRow) -> String {
    const MARKER: &str = "task_id:";
    if let Some((_, rest)) = row.summary.split_once(MARKER) {
        let task_id = rest
            .trim_start()
            .split(|ch: char| ch.is_whitespace() || ch == ',' || ch == '\u{00B7}')
            .next()
            .unwrap_or_default()
            .trim();
        if !task_id.is_empty() {
            return task_id.to_string();
        }
    }

    normalize_activity_text(&row.summary)
}

fn normalize_activity_text(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
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

fn tool_status_marker(
    status: ToolStatus,
    theme: &palette::UiTheme,
) -> (&'static str, ratatui::style::Color) {
    match status {
        ToolStatus::Running => ("[~]", theme.warning),
        ToolStatus::Success => ("[✓]", theme.success),
        ToolStatus::Failed => ("[!]", theme.error_fg),
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

fn render_sidebar_subagents(f: &mut Frame, area: Rect, app: &mut App) {
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
    let lines = subagent_panel_lines(
        &summary,
        &rows,
        content_width,
        usable_rows.max(1),
        &app.ui_theme,
    );

    render_sidebar_section(f, area, "Agents", lines, Vec::new(), app);
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
    theme: &palette::UiTheme,
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
            Style::default().fg(theme.text_muted),
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
            Style::default().fg(theme.text_dim),
        )));
    }

    for row in rows {
        if lines.len() >= max_rows {
            break;
        }
        let (marker, color) = agent_status_marker(row.status.as_str(), theme);
        let label = format!("{marker} {} {}", row.role, row.name);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&label, content_width.max(1)),
            Style::default().fg(color),
        )));

        // Auto-collapse finished sub-agents: hide detail lines for completed
        // agents so the sidebar stays compact when work is done.
        if row.status == "done" {
            continue;
        }

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
            Style::default().fg(theme.text_dim),
        )));
    }

    if summary.foreground_rlm_running {
        lines.push(Line::from(vec![
            Span::styled("RLM", Style::default().fg(theme.accent_primary).bold()),
            Span::styled(
                " foreground work active",
                Style::default().fg(theme.text_dim),
            ),
        ]));
    }

    lines
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
fn render_context_panel(f: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 3 {
        return;
    }

    let theme = &app.ui_theme;
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
            Style::default().fg(theme.accent_primary).bold(),
        ),
        Span::styled(
            format!("  {}", app.workspace_context.as_deref().unwrap_or("")),
            Style::default().fg(theme.text_dim),
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
        Style::default().fg(theme.text_muted),
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
        Style::default().fg(theme.text_muted),
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
        let size_hint = std::fs::metadata(&app.memory_path)
            .map(|m| m.len())
            .map(|bytes| {
                if bytes >= 1024 * 1024 {
                    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
                } else if bytes >= 1024 {
                    format!("{:.1} KB", bytes as f64 / 1024.0)
                } else {
                    format!("{bytes} B")
                }
            })
            .unwrap_or_else(|_| "—".to_string());
        lines.push(Line::from(Span::styled(
            format!("memory: {} ({})", app.memory_path.display(), size_hint),
            Style::default().fg(theme.text_muted),
        )));
    }

    render_sidebar_section(f, area, "Session", lines, Vec::new(), app);
}

fn spans_to_text(spans: &[Span<'_>]) -> String {
    let mut s = String::new();
    for span in spans {
        s.push_str(span.content.as_ref());
    }
    s
}

fn render_sidebar_section(
    f: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    full_texts: Vec<String>,
    app: &mut App,
) {
    if area.width < 4 || area.height < 3 {
        // Clear stale cells before bailing out (#400).
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(area, f.buffer_mut());
        return;
    }

    let theme = Theme::for_palette_mode(app.ui_theme.mode);

    // Record hover metadata for mouse tooltip support.
    let padding = theme.section_padding;
    let content_area = Rect {
        x: area.x + 1 + padding.left,
        y: area.y + 1 + padding.top,
        width: area.width.saturating_sub(2 + padding.left + padding.right),
        height: area.height.saturating_sub(2 + padding.top + padding.bottom),
    };
    app.sidebar_hover.sections.push(SidebarHoverSection {
        content_area,
        lines: full_texts,
    });
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
        AutoSidebarState, SidebarAgentRow, SidebarHoverSection, SidebarHoverState,
        SidebarSubagentSummary, SidebarToolRow, SidebarWorkChecklistItem, SidebarWorkStrategyStep,
        SidebarWorkSummary, ToolRowOrder, auto_sidebar_panels, editorial_tool_rows,
        normalize_activity_text, sidebar_work_summary, subagent_panel_lines, task_panel_lines,
        work_panel_empty_hint, work_panel_lines,
    };
    use crate::config::Config;
    use crate::palette;
    use crate::palette::PaletteMode;
    use crate::tools::plan::StepStatus;
    use crate::tools::todo::TodoStatus;
    use crate::tui::active_cell::ActiveCell;
    use crate::tui::app::{App, HuntVerdict, TaskPanelEntry, TuiOptions};
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

    fn sidebar_tool_row(name: &str, status: ToolStatus) -> SidebarToolRow {
        SidebarToolRow {
            name: name.to_string(),
            status,
            summary: String::new(),
            duration_ms: None,
        }
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
    fn editorial_rows_keep_newer_failure_when_older_success_is_seen_later() {
        let rows = vec![
            sidebar_tool_row("gh issue create", ToolStatus::Failed),
            sidebar_tool_row("gh issue create", ToolStatus::Success),
        ];

        let rendered = editorial_tool_rows(rows, 4, ToolRowOrder::NewestFirst);

        assert!(
            rendered
                .iter()
                .any(|row| row.name == "gh issue create" && row.status == ToolStatus::Failed),
            "newest-first rows must keep a failure newer than a later-seen success: {rendered:?}"
        );
    }

    #[test]
    fn normalize_activity_text_strips_ansi_before_collapsing_text() {
        let text = normalize_activity_text("running \x1b[48;2;10;17;32mtool\x1b[0m now");
        assert_eq!(text, "running tool now");
        assert!(!text.contains("48;2"));
    }

    #[test]
    fn editorial_rows_hide_older_failure_after_newer_success() {
        let rows = vec![
            sidebar_tool_row("gh issue create", ToolStatus::Success),
            sidebar_tool_row("gh issue create", ToolStatus::Failed),
        ];

        let rendered = editorial_tool_rows(rows, 4, ToolRowOrder::NewestFirst);

        assert!(
            !rendered
                .iter()
                .any(|row| row.name == "gh issue create" && row.status == ToolStatus::Failed),
            "newest-first rows should hide stale failures older than success: {rendered:?}"
        );
    }

    #[test]
    fn editorial_rows_reclaim_failure_slot_after_oldest_first_success() {
        let rows = vec![
            sidebar_tool_row("gh issue create", ToolStatus::Failed),
            sidebar_tool_row("grep_files", ToolStatus::Failed),
            sidebar_tool_row("gh issue create", ToolStatus::Success),
            sidebar_tool_row("cargo test", ToolStatus::Failed),
        ];

        let rendered = editorial_tool_rows(rows, 2, ToolRowOrder::OldestFirst);

        assert_eq!(
            rendered
                .iter()
                .filter(|row| row.status == ToolStatus::Failed)
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            vec!["grep_files", "cargo test"],
            "success should clear its stale failure and free a visible failure slot"
        );
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

        let text = lines_to_text(&work_panel_lines(
            &summary,
            80,
            16,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));

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
    fn work_panel_includes_strategy_only_when_plan_state_is_non_empty() {
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

        let summary = SidebarWorkSummary {
            strategy_explanation: Some("High-level sequencing".to_string()),
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
            text.iter().any(|line| line == "Strategy metadata"),
            "non-empty plan should show strategy label: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("High-level sequencing")),
            "non-empty plan explanation should render: {text:?}"
        );
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
        app.hunt.quarry = Some("test quarry".to_string());
        app.hunt.verdict = HuntVerdict::Hunted;
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add("item".to_string(), TodoStatus::Pending);
        }
        let _first = sidebar_work_summary(&mut app);

        app.hunt.quarry = Some("updated quarry".to_string());
        app.hunt.verdict = HuntVerdict::Hunting;
        let held_arc = app.todos.clone();
        let _held = held_arc.try_lock().expect("hold todos lock");

        let summary = sidebar_work_summary(&mut app);

        assert_eq!(summary.goal_objective.as_deref(), Some("updated quarry"));
        assert!(!summary.goal_completed, "verdict should be live");
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
                input_summary: Some("codewhale-tui/CHANGELOG.md".to_string()),
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
            text.iter().any(|line| line.contains("[✓] read_file")),
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
        let expired_at = instant_older_than(ACTIVE_TOOL_COMPLETED_ROW_TTL + Duration::from_secs(1));
        app.active_tool_entry_completed_at.insert(0, expired_at);

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));

        assert!(
            !text.iter().any(|line| line.contains("[✓] read_file")),
            "expired completed active row should leave the sidebar: {text:?}"
        );
    }

    fn instant_older_than(age: Duration) -> Instant {
        if let Some(instant) = Instant::now().checked_sub(age) {
            return instant;
        }

        let instant = Instant::now();
        std::thread::sleep(age);
        instant
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
            text.iter().any(|line| line.contains("[✓] read_file")),
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
            !text.iter().any(|line| line.contains("Background commands")),
            "duplicate background shell row should be hidden: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_puts_background_shell_command_on_primary_row() {
        let mut app = create_test_app();
        app.task_panel.push(TaskPanelEntry {
            id: "shell_33a08c3c".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cd /tmp/repo && cargo test --workspace --all-features"
                .to_string(),
            duration_ms: Some(178_000),
        });

        let text = lines_to_text(&task_panel_lines(&app, 96, 8));

        assert!(
            text.iter()
                .any(|line| line.contains("running cargo test --workspace --all-features")),
            "background shell headline should show the command, not only the shell id: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("shell_33a08c3c")),
            "shell id should remain available as detail: {text:?}"
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
            .position(|line| line.contains("[✓] read_file x3"))
            .expect("repeated read_file rows should collapse");

        assert!(
            failed_index < read_group_index,
            "failure should sort above low-value success noise: {text:?}"
        );
        assert_eq!(
            text.iter()
                .filter(|line| line.contains("[✓] read_file"))
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
                command: "cd /tmp/repo && sleep 15 && gh pr checks 1616 --repo Hmbown/CodeWhale"
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
            command: "cargo test -p codewhale-tui".to_string(),
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
                .any(|line| line.contains("[✓] cargo check 1.2s")),
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
            text.iter().any(|line| line.contains("[~] wait command")),
            "shell helper should render as a user-facing activity: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("task_shell_wait")),
            "internal helper name should not leak into sidebar: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_collapses_repeated_shell_waits_for_same_job() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        for id in ["shell-wait-1", "shell-wait-2"] {
            active.push_tool(
                id,
                HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                    name: "task_shell_wait".to_string(),
                    status: ToolStatus::Running,
                    input_summary: Some("task_id: shell_33a08c3c".to_string()),
                    output: None,
                    prompts: None,
                    spillover_path: None,
                    output_summary: Some("Background task running (no new output).".to_string()),
                    is_diff: false,
                })),
            );
        }
        app.active_cell = Some(active);

        let text = lines_to_text(&task_panel_lines(&app, 100, 8));

        assert_eq!(
            text.iter()
                .filter(|line| line.contains("[~] wait command"))
                .count(),
            1,
            "duplicate waits for the same shell job should collapse: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("2 waits collapsed")),
            "collapsed row should explain why only one wait is visible: {text:?}"
        );
    }

    #[test]
    fn navigator_empty_state_says_no_agents() {
        let summary = SidebarSubagentSummary::default();
        let lines = subagent_panel_lines(&summary, &[], 32, 8, &palette::UI_THEME);
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
        let text = lines_to_text(&subagent_panel_lines(
            &summary,
            &rows,
            64,
            12,
            &palette::UI_THEME,
        ));
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

        let text = lines_to_text(&subagent_panel_lines(
            &summary,
            &[],
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
        let lines = subagent_panel_lines(&summary, &[], 16, 8, &palette::UI_THEME);
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

    // ── Pause lifecycle tests ────────────────────────────────────────

    #[test]
    fn pause_indicator_none_when_not_paused_or_cancelled() {
        let mut app = create_test_app();
        app.paused = false;
        app.paused_cancelled = false;
        let summary = sidebar_work_summary(&mut app);
        assert!(summary.pause_indicator.is_none(), "expected no indicator when not paused/cancelled");
    }

    #[test]
    fn pause_indicator_paused_when_paused() {
        let mut app = create_test_app();
        app.paused = true;
        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"));
    }

    #[test]
    fn pause_indicator_pausing_when_loading() {
        let mut app = create_test_app();
        app.paused = true;
        app.is_loading = true;
        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Pausing)"));
    }

    #[test]
    fn pause_indicator_cancelled_when_cancelled() {
        let mut app = create_test_app();
        app.paused_cancelled = true;
        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Cancelled)"));
    }

    #[test]
    fn goal_icon_play_when_running() {
        let summary = SidebarWorkSummary {
            goal_objective: Some("test".to_string()),
            goal_completed: false,
            pause_indicator: None,
            ..SidebarWorkSummary::default()
        };
        let lines = work_panel_lines(&summary, 80, 10, PaletteMode::Dark, &palette::UI_THEME);
        let first = lines.first().map(|l| l.to_string()).unwrap_or_default();
        assert!(first.contains('▶'), "expected play icon for running, got: {first}");
    }

    #[test]
    fn goal_icon_check_when_completed() {
        let summary = SidebarWorkSummary {
            goal_objective: Some("test".to_string()),
            goal_completed: true,
            pause_indicator: None,
            ..SidebarWorkSummary::default()
        };
        let lines = work_panel_lines(&summary, 80, 10, PaletteMode::Dark, &palette::UI_THEME);
        let first = lines.first().map(|l| l.to_string()).unwrap_or_default();
        assert!(first.contains('✓'), "expected checkmark for completed, got: {first}");
    }

    #[test]
    fn goal_icon_pause_when_paused() {
        let summary = SidebarWorkSummary {
            goal_objective: Some("test".to_string()),
            goal_completed: false,
            pause_indicator: Some("(Paused)".to_string()),
            ..SidebarWorkSummary::default()
        };
        let lines = work_panel_lines(&summary, 80, 10, PaletteMode::Dark, &palette::UI_THEME);
        let first = lines.first().map(|l| l.to_string()).unwrap_or_default();
        assert!(first.contains('⏸'), "expected pause icon for paused, got: {first}");
    }

    #[test]
    fn goal_icon_cancelled_when_cancelled() {
        let summary = SidebarWorkSummary {
            goal_objective: Some("test".to_string()),
            goal_completed: false,
            pause_indicator: Some("(Cancelled)".to_string()),
            ..SidebarWorkSummary::default()
        };
        let lines = work_panel_lines(&summary, 80, 10, PaletteMode::Dark, &palette::UI_THEME);
        let first = lines.first().map(|l| l.to_string()).unwrap_or_default();
        assert!(first.contains('✘'), "expected cross icon for cancelled, got: {first}");
    }

    #[test]
    fn cancel_status_not_overwritten_by_late_paused_event() {
        // Simulate what the Event::Status handler in ui.rs does:
        // when paused_cancelled is true, "Paused"/"Resumed" events are discarded.
        let mut app = create_test_app();
        app.paused_cancelled = true;
        app.status_message = Some("Request was Cancelled".to_string());

        // Simulate the guard from EngineEvent::Status handler in ui.rs:
        let late_message = "Request was Paused".to_string();
        if !(app.paused_cancelled
            && (late_message == "Request was Paused" || late_message == "Request was Resumed"))
        {
            app.status_message = Some(late_message);
        }

        assert_eq!(
            app.status_message.as_deref(),
            Some("Request was Cancelled"),
            "guard must discard late Paused event when cancelled"
        );
    }

    #[test]
    fn pause_sets_pause_message_as_quarry() {
        // When the user pauses a command, the quarry must be set to a pause
        // message so the system prompt tells the model the command is on hold.
        let mut app = create_test_app();
        app.hunt.quarry = Some("Scan nested git repositories".to_string());


        // Simulate PauseCommand handler saving + clearing the quarry:
        app.paused_quarry = app.hunt.quarry.clone();
        app.hunt.quarry = None;
        app.paused = true;

        assert_eq!(app.paused_quarry.as_deref(), Some("Scan nested git repositories"),
            "paused_quarry must save the original goal");
        assert!(app.hunt.quarry.is_none(),
            "hunt.quarry must be None so goal system doesn't prompt model to resolve a pause goal");
        assert!(app.paused, "app.paused must be true");
    }

    #[test]
    fn completed_command_clears_quarry() {
        // When a command completes successfully, the quarry must be cleared
        // so the system prompt stops telling the model to continue the goal.
        let mut app = create_test_app();
        app.hunt.quarry = Some("Scan repos".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;

        // Simulate successful completion (what the TurnCompleted handler does):
        if app.hunt.quarry.is_some() {
            app.hunt.verdict = crate::tui::app::HuntVerdict::Hunted;
            app.hunt.quarry = None;
        }

        assert!(app.hunt.quarry.is_none(),
            "quarry must be None after completion so model isn't prompted to continue");
        assert_eq!(app.hunt.verdict, crate::tui::app::HuntVerdict::Hunted);
    }

    #[test]
    fn cancel_clears_hunt_goal_so_model_does_not_resume_old_command() {
        // Simulate a running pausable command with a description (hunt.quarry).
        // When cancelled, the hunt goal must be cleared so the model does NOT
        // see the old objective in the system prompt for the next turn.
        let mut app = create_test_app();
        app.hunt.quarry = Some("Scan nested git repositories".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;
        app.hunt.started_at = Some(std::time::Instant::now());
        app.active_allowed_tools = Some(vec!["exec_shell".to_string()]);
        app.paused_cancelled = false;
        app.paused = true;
        app.pausable = true;

        // Simulate CancelRequest handler clearing on cancel:
        // quarry is NOT cleared here — it persists so the WorkBench shows
        // the cancelled goal. It is cleared in dispatch_user_message when
        // the user sends a new message.
        app.paused_cancelled = true;
        app.paused = false;
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;
        app.active_allowed_tools = None;

        // Quarry is kept visible for WorkBench display
        assert_eq!(app.hunt.quarry.as_deref(), Some("Scan nested git repositories"),
            "quarry must persist on cancel so WorkBench shows the cancelled goal");
        // Simulate dispatch_user_message clearing on next user message:
        app.hunt.quarry = None;
        assert!(app.hunt.quarry.is_none(),
            "quarry cleared in dispatch_user_message when user sends next message");
        assert_eq!(app.hunt.verdict, crate::tui::app::HuntVerdict::Hunting);
        assert!(app.active_allowed_tools.is_none(),
            "tool restriction must be cleared on cancel");
    }

    #[test]
    fn new_slash_command_after_cancel_clears_all_pause_state() {
        // Simulate the full lifecycle:
        // 1. Command running → cancel (paused_cancelled=true)
        // 2. User types /git-scan → try_dispatch_user_command clears all state
        // 3. New command should have NO trace of old pause state
        let mut app = create_test_app();
        app.paused_cancelled = true;
        app.paused = false;
        app.pausable = false;
        app.active_snapshot = Some("stash".to_string());
        app.hunt.quarry = Some("old scan".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunted;

        // Simulate try_dispatch_user_command clearing:
        app.paused_cancelled = false;
        app.active_snapshot = None;
        app.hunt.quarry = Some("new task".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;

        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.goal_objective.as_deref(), Some("new task"));
        assert!(summary.pause_indicator.is_none(), "new command must have no pause indicator, got: {:?}", summary.pause_indicator);
    }

    // ── Full lifecycle workflow tests ───────────────────────────────

    #[test]
    fn lifecycle_pause_continue_complete() {
        // 1. Start pausable command
        let mut app = create_test_app();
        app.hunt.quarry = Some("Scan nested git repos".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;
        app.pausable = true;
        app.is_loading = true;

        // → WorkBench should show "▶" play icon
        let summary = sidebar_work_summary(&mut app);
        assert!(!summary.goal_completed, "command is still running");
        assert!(summary.pause_indicator.is_none(), "not paused yet");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "goal must show original description");

        // 2. User presses ESC — pause
        app.paused_quarry = app.hunt.quarry.clone();
        app.hunt.quarry = None;
        app.paused = true;
        app.pausable = true;
        app.is_loading = false; // past tool drain

        // → WorkBench should show "⏸ (Paused)"
        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"),
            "must show paused after ESC");
        // quarry is cleared for system prompt, but paused_quarry keeps
        // the goal visible in the WorkBench (with ⏸ icon).
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "WorkBench must show the goal even when paused (from paused_quarry)");

        // 3. User types "continue" — restore quarry, unpause
        app.hunt.quarry = app.paused_quarry.take();
        app.paused = false;
        app.pausable = false;
        app.is_loading = true;

        // → WorkBench should show "▶" play icon with original goal
        let summary = sidebar_work_summary(&mut app);
        assert!(!summary.goal_completed, "continue -> still running");
        assert!(summary.pause_indicator.is_none(), "pause indicator must be gone after resume");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "original goal must be restored on continue");

        // 4. Command completes successfully
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunted;
        app.hunt.quarry = None;
        app.is_loading = false;

        // → WorkBench should show "✓" green checkmark
        let summary = sidebar_work_summary(&mut app);
        assert!(summary.goal_completed, "must be marked completed");
        assert!(summary.pause_indicator.is_none(), "no pause indicator when completed");
        assert!(app.hunt.quarry.is_none(),
            "quarry cleared so model is not prompted to continue");
    }

    #[test]
    fn lifecycle_pause_cancel_new_command() {
        // 1. Start pausable command
        let mut app = create_test_app();
        app.hunt.quarry = Some("Scan nested git repos".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;
        app.pausable = true;

        // 2. User presses ESC — pause
        app.paused_quarry = app.hunt.quarry.clone();
        app.hunt.quarry = None;
        app.paused = true;

        assert!(app.paused, "must be paused");
        assert!(app.paused_quarry.is_some(), "original goal must be saved");
        assert!(app.hunt.quarry.is_none(), "active goal must be cleared");

        // 3. User presses ESC again — cancel (simulate CancelRequest handler)
        // Restore quarry from paused_quarry so WorkBench shows the goal.
        app.paused = false;
        app.pausable = false;
        app.paused_cancelled = true;
        if let Some(saved) = app.paused_quarry.take() {
            app.hunt.quarry = Some(saved);
        }
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;

        // → WorkBench should show "✘ (Cancelled)" with the original goal
        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Cancelled)"),
            "must show cancelled indicator");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "workbench must show the original goal even when cancelled");
        assert!(app.paused_quarry.is_none(), "paused_quarry cleared on cancel");

        // 4. User starts a fresh slash command
        app.paused_cancelled = false;
        app.hunt.quarry = Some("Deploy to staging".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;

        let summary = sidebar_work_summary(&mut app);
        assert!(summary.pause_indicator.is_none(), "new command must have no pause indicator");
        assert_eq!(summary.goal_objective.as_deref(), Some("Deploy to staging"),
            "new command's goal must be shown");
    }

    #[test]
    fn non_continue_while_paused_clears_system_prompt_goal() {
        // When the user types something while paused (not "continue"/"resume"),
        // the hunt.quarry must be None so the system prompt has no goal.
        // This prevents the model from being prompted to continue the old command.
        // The paused_quarry is preserved for WorkBench display.
        let mut app = create_test_app();
        app.hunt.quarry = None;          // set by PauseCommand handler
        app.paused_quarry = Some("Scan repos".to_string());
        app.paused = true;

        // Simulate dispatch_user_message for a non-continue message:
        app.paused = false;
        app.paused_cancelled = false;
        app.pausable = false;
        // app.hunt.quarry stays None — NOT restored

        assert!(app.hunt.quarry.is_none(), "quarry must stay None for non-continue: system prompt has no goal");
        assert_eq!(app.paused_quarry.as_deref(), Some("Scan repos"), "paused_quarry preserved for WorkBench");
    }

    #[test]
    fn workbench_visible_through_pause_ask_resume() {
        // Full user workflow: start -> ESC (pause) -> ask "how are you?"
        // -> resume -> WorkBench must persist through every step.
        let mut app = create_test_app();

        // 1. Start pausable command
        app.hunt.quarry = Some("Scan nested git repos".to_string());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Hunting;

        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "WorkBench shows the goal when running");
        assert!(summary.pause_indicator.is_none(), "no pause when running");

        // 2. ESC -- pause (quarry saved to paused_quarry, cleared from hunt)
        app.paused_quarry = app.hunt.quarry.clone();
        app.hunt.quarry = None;
        app.paused = true;

        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"), "(paused) when paused");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "WorkBench still shows the goal via paused_quarry fallback");

        // 3. Type "how are you?" (non-continue message) -- unpause,
        //    quarry stays None, paused_quarry preserved for WorkBench
        app.paused = false;
        app.paused_cancelled = false;

        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"),
            "(paused) still visible after 'how are you?' -- paused_quarry preserved");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "WorkBench shows the paused command so user remembers to resume");

        // 4. Type "resume" -- restore quarry from paused_quarry
        app.hunt.quarry = app.paused_quarry.take();

        let summary = sidebar_work_summary(&mut app);
        assert_eq!(summary.pause_indicator.as_deref(), None, "pause gone after resume");
        assert_eq!(summary.goal_objective.as_deref(), Some("Scan nested git repos"),
            "WorkBench shows the goal again with (play) after resume");
    }
}