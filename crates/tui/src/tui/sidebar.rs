//! Sidebar rendering — Plan / Todos / Tasks / Agents panels.
//!
//! Extracted from `tui/ui.rs` (P1.2). The sidebar appears to the right of
//! the chat transcript when the available width allows it. Each section
//! reads from `App` snapshots; mutation lives in the main app loop.

use std::fmt::Write;

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

use super::app::{App, SidebarFocus};
use super::history::{HistoryCell, ToolCell, ToolStatus};
use super::subagent_routing::active_fanout_counts;
use super::ui::truncate_line_to_width;

/// Tolerance for floating-point cost comparison in the sidebar breakdown.
/// Must be large enough that accumulated f64 error across hundreds of turns
/// does not prematurely hide the session+agents breakdown.
const COST_EQ_TOLERANCE: f64 = 1e-6;

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
        SidebarFocus::Plan => render_sidebar_plan(f, area, app),
        SidebarFocus::Todos => render_sidebar_todos(f, area, app),
        SidebarFocus::Tasks => render_sidebar_tasks(f, area, app),
        SidebarFocus::Agents => render_sidebar_subagents(f, area, app),
        SidebarFocus::Context => render_context_panel(f, area, app),
    }
}

/// Build the Auto-mode panel stack. Empty panels collapse to zero height so
/// non-empty ones get the full sidebar real estate. Without this, Plan got
/// clipped because Todos/Tasks/Agents each reserved 25% of the height even
/// when they had nothing to show. Plan is always rendered (it owns the
/// session-wide empty-state hint).
fn render_sidebar_auto(f: &mut Frame, area: Rect, app: &App) {
    #[derive(Clone, Copy)]
    enum Panel {
        Plan,
        Todos,
        Tasks,
        Agents,
        Context,
    }

    let todos_empty = app
        .todos
        .try_lock()
        .map(|todos| todos.snapshot().items.is_empty())
        .unwrap_or(false); // assume non-empty when locked so we don't hide updating data
    let tasks_empty = app.runtime_turn_id.is_none() && app.task_panel.is_empty();
    let agents_empty = app.subagent_cache.is_empty()
        && app.agent_progress.is_empty()
        && active_fanout_counts(app).is_none()
        && !foreground_rlm_running(app);

    let mut visible: Vec<Panel> = Vec::with_capacity(5);
    visible.push(Panel::Plan);
    if !todos_empty {
        visible.push(Panel::Todos);
    }
    if !tasks_empty {
        visible.push(Panel::Tasks);
    }
    if !agents_empty {
        visible.push(Panel::Agents);
    }
    if app.context_panel {
        visible.push(Panel::Context);
    }

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
            Panel::Plan => render_sidebar_plan(f, *rect, app),
            Panel::Todos => render_sidebar_todos(f, *rect, app),
            Panel::Tasks => render_sidebar_tasks(f, *rect, app),
            Panel::Agents => render_sidebar_subagents(f, *rect, app),
            Panel::Context => render_context_panel(f, *rect, app),
        }
    }
}

/// The Plan section is the **single source of truth for the
/// `update_plan` tool's output** (#408). It is intentionally distinct
/// from the Todos section: todos are checklist work items the user
/// or model is tracking; plan steps are the model's higher-level
/// strategy as recorded by `update_plan`. The panel also hosts two
/// session-wide indicators that don't fit the other sections — Goal
/// (`/goal`) and the cycle counter (#124) — because they share the
/// "what's the agent trying to do, big-picture" theme.
///
/// When the panel is fully empty (no goal, no cycles, no plan) it
/// renders as a quiet section with a single dim hint at the bottom
/// rather than the blunt "No active plan" placeholder it used to show.
/// That kept the user wondering whether the panel was broken; the
/// hint instead tells them what the panel is for and how to populate
/// it.
fn render_sidebar_plan(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let theme = Theme::for_palette_mode(app.ui_theme.mode);
    let content_width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(usize::from(area.height).max(4));

    // === Goal Mode (#397) — gold outline matching todo items ===
    if let Some(ref objective) = app.goal.goal_objective {
        lines.push(Line::from(Span::styled(
            format!(
                "◆ {}",
                truncate_line_to_width(objective, content_width.max(1))
            ),
            Style::default()
                .fg(palette::STATUS_WARNING)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )));
        if let Some(budget) = app.goal.goal_token_budget {
            let used = app.session.total_conversation_tokens;
            let pct = if budget > 0 {
                ((used as f64 / budget as f64) * 100.0).min(100.0)
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
                format!("  tokens: {used}/{budget} {}", bar),
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
        // Gold separator
        lines.push(Line::from(Span::styled(
            "─".repeat(content_width.min(24)),
            Style::default().fg(palette::STATUS_WARNING),
        )));
    }

    // Cycle indicator (issue #124). Only shown once a boundary has fired —
    // first-time users with cycle_count == 0 don't need this row of chrome.
    if app.cycle_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "cycles: {} (active: {})",
                app.cycle_count,
                app.cycle_count.saturating_add(1)
            ),
            Style::default().fg(theme.plan_summary_color),
        )));
    }

    match app.plan_state.try_lock() {
        Ok(plan) => {
            if plan.is_empty() {
                // The blunt "No active plan" placeholder used to land
                // here on every render with no plan steps, even when the
                // user had a goal set or had cycled — making the panel
                // look broken. After #408 we instead emit a quiet hint
                // that explains what the panel is for, but only when
                // *all* of the panel's signals are empty so we don't
                // crowd a panel that already has a goal / cycle
                // indicator above.
                let nothing_above = app.goal.goal_objective.is_none() && app.cycle_count == 0;
                if nothing_above {
                    lines.push(Line::from(Span::styled(
                        plan_panel_empty_hint(content_width.max(1)),
                        Style::default().fg(palette::TEXT_MUTED).italic(),
                    )));
                }
            } else {
                let (pending, in_progress, completed) = plan.counts();
                let total = pending + in_progress + completed;
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}%", plan.progress_percent()),
                        Style::default().fg(theme.plan_progress_color).bold(),
                    ),
                    Span::styled(
                        format!(" complete ({completed}/{total})"),
                        Style::default().fg(theme.plan_summary_color),
                    ),
                ]));

                if let Some(explanation) = plan.explanation() {
                    lines.push(Line::from(Span::styled(
                        truncate_line_to_width(explanation, content_width.max(1)),
                        Style::default().fg(theme.plan_explanation_color),
                    )));
                }

                let usable_rows = area.height.saturating_sub(3) as usize;
                let max_steps = usable_rows.saturating_sub(lines.len());
                for step in plan.steps().iter().take(max_steps) {
                    let (prefix, color) = match &step.status {
                        StepStatus::Pending => ("[ ]", theme.plan_pending_color),
                        StepStatus::InProgress => ("[~]", theme.plan_in_progress_color),
                        StepStatus::Completed => ("[x]", theme.plan_completed_color),
                    };
                    let mut text = format!("{prefix} {}", step.text);
                    let elapsed = step.elapsed_str();
                    if !elapsed.is_empty() {
                        let _ = write!(text, " ({elapsed})");
                    }
                    lines.push(Line::from(Span::styled(
                        truncate_line_to_width(&text, content_width.max(1)),
                        Style::default().fg(color),
                    )));
                }

                let remaining = plan.steps().len().saturating_sub(max_steps);
                if remaining > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("+{remaining} more steps"),
                        Style::default().fg(theme.plan_summary_color),
                    )));
                }
            }
        }
        Err(_) => {
            lines.push(Line::from(Span::styled(
                "Plan state updating...",
                Style::default().fg(theme.plan_summary_color),
            )));
        }
    }

    render_sidebar_section(f, area, "Plan", lines, app);
}

/// One-line hint shown when the Plan section has nothing to display
/// (no goal, no cycle, no steps). Ellipsizes for narrow widths so
/// even a 24-column sidebar doesn't wrap mid-word. Visible across
/// modes — the panel's role doesn't change between Plan / Agent /
/// YOLO; only its content does.
#[must_use]
fn plan_panel_empty_hint(content_width: usize) -> String {
    let full = "tracks update_plan / /goal / cycles";
    truncate_line_to_width(full, content_width)
}

fn render_sidebar_todos(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(usize::from(area.height).max(4));

    match app.todos.try_lock() {
        Ok(todos) => {
            let snapshot = todos.snapshot();
            if snapshot.items.is_empty() {
                lines.push(Line::from(Span::styled(
                    "No todos",
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            } else {
                let total = snapshot.items.len();
                let completed = snapshot
                    .items
                    .iter()
                    .filter(|item| item.status == TodoStatus::Completed)
                    .count();
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{}%", snapshot.completion_pct),
                        Style::default().fg(palette::STATUS_SUCCESS).bold(),
                    ),
                    Span::styled(
                        format!(" complete ({completed}/{total})"),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]));

                let usable_rows = area.height.saturating_sub(3) as usize;
                let max_items = usable_rows.saturating_sub(lines.len());
                for item in snapshot.items.iter().take(max_items) {
                    let (prefix, color) = match item.status {
                        TodoStatus::Pending => ("[ ]", palette::TEXT_MUTED),
                        TodoStatus::InProgress => ("[~]", palette::STATUS_WARNING),
                        TodoStatus::Completed => ("[x]", palette::STATUS_SUCCESS),
                    };
                    let text = format!("{prefix} #{} {}", item.id, item.content);
                    lines.push(Line::from(Span::styled(
                        truncate_line_to_width(&text, content_width.max(1)),
                        Style::default().fg(color),
                    )));
                }

                let remaining = snapshot.items.len().saturating_sub(max_items);
                if remaining > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("+{remaining} more todos"),
                        Style::default().fg(palette::TEXT_MUTED),
                    )));
                }
            }
        }
        Err(_) => {
            lines.push(Line::from(Span::styled(
                "Todo list updating...",
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }
    }

    render_sidebar_section(f, area, "Todos", lines, app);
}

fn render_sidebar_tasks(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(usize::from(area.height).max(4));

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

    if app.task_panel.is_empty() {
        lines.push(Line::from(Span::styled(
            "No active tasks",
            Style::default().fg(palette::TEXT_MUTED),
        )));
    } else {
        let running = app
            .task_panel
            .iter()
            .filter(|task| task.status == "running")
            .count();
        lines.push(Line::from(vec![
            Span::styled(
                if running == app.task_panel.len() {
                    format!("{running} running")
                } else {
                    format!("{} active", app.task_panel.len())
                },
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            ),
            Span::styled(
                if running == app.task_panel.len() {
                    String::new()
                } else {
                    format!(" ({running} running)")
                },
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]));

        let usable_rows = area.height.saturating_sub(3) as usize;
        let max_items = usable_rows.saturating_sub(lines.len());
        for task in app.task_panel.iter().take(max_items) {
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
                .map(|ms| format!("{:.1}s", ms as f64 / 1000.0))
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
    }

    render_sidebar_section(f, area, "Tasks", lines, app);
}

fn render_sidebar_subagents(f: &mut Frame, area: Rect, app: &App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;

    // Demoted to navigator (issue #128): the in-transcript DelegateCard /
    // FanoutCard now carries the live action tree and dot-grid. The sidebar
    // shows just count + role-mix so the user can scan parallel work at a
    // glance and scroll to the matching transcript card for detail.
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
    let lines = subagent_navigator_lines(&summary, content_width);

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

fn foreground_rlm_running(app: &App) -> bool {
    app.active_cell.as_ref().is_some_and(|active| {
        active.entries().iter().any(|entry| {
            matches!(
                entry,
                HistoryCell::Tool(ToolCell::Generic(generic))
                    if generic.name == "rlm" && generic.status == ToolStatus::Running
            )
        })
    })
}

/// Build the demoted navigator lines from a summary projection. Public
/// for the snapshot test in this module.
pub fn subagent_navigator_lines(
    summary: &SidebarSubagentSummary,
    content_width: usize,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(4);

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

    if summary.foreground_rlm_running {
        lines.push(Line::from(vec![
            Span::styled("RLM", Style::default().fg(palette::DEEPSEEK_SKY).bold()),
            Span::styled(
                " foreground work active",
                Style::default().fg(palette::TEXT_DIM),
            ),
        ]));
    }

    lines.push(Line::from(Span::styled(
        "(see transcript card for detail)",
        Style::default().fg(palette::TEXT_MUTED).italic(),
    )));

    lines
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
    let total_cost = app.displayed_session_cost_for_currency(app.cost_currency);
    let session_cost = app.session_cost_for_currency(app.cost_currency);
    let agent_cost = app.subagent_cost_for_currency(app.cost_currency);
    let real_total = session_cost + agent_cost;
    // Only show the additive breakdown when it matches the displayed
    // total; when the high-water mark is in effect (post-reconciliation),
    // the breakdown would not sum to the displayed value (#244).
    let cost_line = if (total_cost - real_total).abs() < COST_EQ_TOLERANCE {
        format!(
            "cost: {} (session {} + agents {})",
            app.format_cost_amount(total_cost),
            app.format_cost_amount(session_cost),
            app.format_cost_amount(agent_cost)
        )
    } else {
        format!("cost: {}", app.format_cost_amount(total_cost),)
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
    use super::{SidebarSubagentSummary, plan_panel_empty_hint, subagent_navigator_lines};
    use ratatui::text::Line;

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

    // ---- #408 Plan panel empty-state hint ----

    #[test]
    fn plan_panel_empty_hint_mentions_panels_role() {
        // The hint replaces the old "No active plan" placeholder; it
        // should explain what the panel tracks so the user can tell
        // whether the panel is broken vs simply unused this turn.
        let hint = plan_panel_empty_hint(80);
        assert!(
            hint.contains("update_plan"),
            "hint should name the tool: {hint:?}"
        );
        assert!(
            hint.contains("/goal") || hint.contains("goal"),
            "hint should mention /goal: {hint:?}"
        );
    }

    #[test]
    fn plan_panel_empty_hint_truncates_to_narrow_widths() {
        // Width 16 forces an ellipsis; the hint should still fit.
        let hint = plan_panel_empty_hint(16);
        assert!(
            hint.chars().count() <= 16,
            "hint width {} > 16: {hint:?}",
            hint.chars().count()
        );
    }

    #[test]
    fn plan_panel_empty_hint_does_not_say_no_active_plan() {
        // Regression guard: the placeholder used to say "No active
        // plan" which made the panel look broken. The hint should
        // never re-introduce that wording.
        let hint = plan_panel_empty_hint(80);
        assert!(
            !hint.to_ascii_lowercase().contains("no active plan"),
            "hint regressed to old placeholder: {hint:?}"
        );
    }

    #[test]
    fn navigator_empty_state_says_no_agents() {
        let summary = SidebarSubagentSummary::default();
        let lines = subagent_navigator_lines(&summary, 32);
        let text = lines_to_text(&lines);
        assert_eq!(text, vec!["No agents".to_string()]);
    }

    #[test]
    fn navigator_running_state_renders_count_role_and_navigator_hint() {
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
        let text = lines_to_text(&subagent_navigator_lines(&summary, 64));
        assert!(text[0].contains("2 running"), "header: {:?}", text[0]);
        assert!(text[0].contains("/ 3"), "total in header: {:?}", text[0]);
        assert!(
            text[1].contains("1 explore") && text[1].contains("2 general"),
            "role mix line: {:?}",
            text[1]
        );
        assert!(
            text.iter().any(|l| l.contains("transcript card")),
            "navigator hint must defer to transcript: {text:?}",
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

        let text = lines_to_text(&subagent_navigator_lines(&summary, 64));

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
        let text = lines_to_text(&subagent_navigator_lines(&summary, 32));
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
        let lines = subagent_navigator_lines(&summary, 16);
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
        let text = lines_to_text(&subagent_navigator_lines(&summary, 64));

        assert!(!text[0].contains("No agents"), "header: {:?}", text);
        assert!(
            text.iter()
                .any(|line| line.contains("RLM foreground work active")),
            "RLM work must be visible in Agents panel: {text:?}"
        );
    }
}
