//! Sidebar rendering — Pinned / Tasks / Agents / Context panels.
//!
//! Extracted from `tui/ui.rs` (P1.2). The sidebar appears to the right of
//! the chat transcript when the available width allows it. Each section
//! reads from `App` snapshots; mutation lives in the main app loop.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::time::{Duration, Instant};

use crate::config::Config;
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
use crate::tools::subagent::{AgentWorkerStatus, SubAgentStatus, agent_worker_status_name};
use crate::tools::todo::TodoStatus;

use super::app::{
    App, SidebarFocus, SidebarHoverRow, SidebarHoverSection, SidebarHoverState, TaskPanelEntry,
    TaskPanelEntryKind,
};
use super::history::{GenericToolCell, HistoryCell, ToolCell, ToolStatus, summarize_tool_output};
use super::spinner::braille_spinner_frame_for_duration_ms;
use super::subagent_routing::active_fanout_counts;
use super::ui_text::{concise_shell_command_label, truncate_line_to_width};

/// Tolerance for floating-point cost comparison in the sidebar breakdown.
/// Must be large enough that accumulated f64 error across hundreds of turns
/// does not prematurely hide the session+agents breakdown.
const COST_EQ_TOLERANCE: f64 = 1e-6;
const RECENT_TOOL_SCAN_LIMIT: usize = 24;
const ACTIVE_TOOL_COMPLETED_ROW_TTL: Duration = Duration::from_secs(8);
const ACTIVE_TOOL_STALE_RUNNING_ROW_TTL: Duration = Duration::from_secs(600);
const TASK_STOP_TARGET_LABEL: &str = "[x]";
const TASK_STOP_TARGET_SUFFIX: &str = " [x]";
const HOTBAR_PANEL_HEIGHT: u16 = 4;
const HOTBAR_ROW_COLUMNS: usize = 4;

pub fn render_sidebar(f: &mut Frame, area: Rect, app: &mut App, config: &Config) {
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

    if app.sidebar_focus == SidebarFocus::Hidden {
        Block::default()
            .style(Style::default().bg(app.ui_theme.surface_bg))
            .render(area, f.buffer_mut());
        return;
    }

    let hotbar_enabled = hotbar_panel_enabled(app, config) && !is_hotbar_disabled(config);
    let (main_area, hotbar_area) = split_sidebar_hotbar_area(area, hotbar_enabled);
    match app.sidebar_focus {
        SidebarFocus::Auto => render_sidebar_auto(f, main_area, app),
        SidebarFocus::Pinned => render_sidebar_pinned(f, main_area, app),
        SidebarFocus::Tasks => render_sidebar_tasks(f, main_area, app),
        SidebarFocus::Agents => render_sidebar_subagents(f, main_area, app),
        SidebarFocus::Context => render_context_panel(f, main_area, app),
        SidebarFocus::Hidden => unreachable!("hidden sidebar returned before render dispatch"),
    }
    if let Some(hotbar_area) = hotbar_area {
        render_hotbar_panel(f, hotbar_area, app, config);
    }
}

fn split_sidebar_hotbar_area(area: Rect, show_hotbar: bool) -> (Rect, Option<Rect>) {
    // Hide the Hotbar entirely when the user disabled it (`hotbar = []`) or when
    // the sidebar is too short to fit it; give the main panel the full area.
    if !show_hotbar || area.height < HOTBAR_PANEL_HEIGHT.saturating_add(3) {
        return (area, None);
    }

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(HOTBAR_PANEL_HEIGHT)])
        .split(area);
    (sections[0], Some(sections[1]))
}

/// The Hotbar is "disabled" when the user persisted an explicit empty
/// `hotbar = []`. Since #3807 a missing `hotbar` key (`None`) also renders no
/// panel — the Hotbar is hidden until the user opts in — but it resolves to
/// zero bindings via [`hotbar_panel_enabled`] rather than the explicit-disabled
/// state, which keeps `/hotbar on` (write default bindings) and `/hotbar off`
/// (write `[]`) distinct on disk.
fn is_hotbar_disabled(config: &Config) -> bool {
    config.hotbar.as_deref().is_some_and(<[_]>::is_empty)
}

/// Build the Auto-mode panel stack. Empty panels collapse to zero height so
/// non-empty ones get the full sidebar real estate. To-do appears when it has
/// useful content, or as the one quiet empty state when nothing else is active.
fn render_sidebar_auto(f: &mut Frame, area: Rect, app: &mut App) {
    let visible = auto_sidebar_panels(auto_sidebar_state(app));
    render_sidebar_panel_stack(f, area, app, &visible);
}

/// Build the pinned panel stack. This uses the same content-sensitive panels
/// as Auto, but it never participates in idle auto-collapse.
fn render_sidebar_pinned(f: &mut Frame, area: Rect, app: &mut App) {
    let visible = auto_sidebar_panels(auto_sidebar_state(app));
    render_sidebar_panel_stack(f, area, app, &visible);
}

fn render_sidebar_panel_stack(
    f: &mut Frame,
    area: Rect,
    app: &mut App,
    visible: &[AutoSidebarPanel],
) {
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

/// Compute the Auto-mode panel signals. Shared by `render_sidebar_auto` (which
/// panel boxes to show) and `sidebar_auto_idle` (whether to collapse the whole
/// sidebar to a full-width transcript). Content-gated: the jobs/tasks panel
/// appears only when there are real durable tasks or background shell jobs,
/// never merely because a turn is in flight.
fn auto_sidebar_state(app: &mut App) -> AutoSidebarState {
    AutoSidebarState {
        work_has_content: sidebar_work_summary(app).has_useful_content(),
        // The jobs/tasks panel appears in Auto mode only for live background
        // work — running or queued shell jobs, RLM, or durable Fleet tasks.
        // Completed jobs, per-turn tools, and model reasoning do not reopen
        // the panel; they remain visible only when Tasks is explicitly focused.
        tasks_empty: !app.task_panel.iter().any(background_task_is_live),
        agents_empty: app.subagent_cache.is_empty()
            && app.agent_progress.is_empty()
            && active_fanout_counts(app).is_none()
            && !foreground_rlm_running(app),
        context_enabled: app.context_panel,
    }
}

/// Auto-reveal: in Auto focus mode the sidebar collapses to nothing when there
/// is no active content (no To-do, no live/queued fleet, no background jobs, no
/// pinned context), so an idle session gets a full-width transcript. Any active
/// content brings it back; completed agents linger in the cache as a natural
/// grace before it retracts. Explicit panel focus and Hidden bypass this (the
/// former should always show, the latter is handled by the width helper).
pub(crate) fn sidebar_auto_idle(app: &mut App) -> bool {
    if app.sidebar_focus != SidebarFocus::Auto {
        return false;
    }
    let state = auto_sidebar_state(app);
    !state.work_has_content && state.tasks_empty && state.agents_empty && !state.context_enabled
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HotbarSlotState {
    Empty,
    Inactive,
    Active,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HotbarPanelSlot {
    slot: u8,
    label: String,
    full_text: String,
    state: HotbarSlotState,
}

fn render_hotbar_panel(f: &mut Frame, area: Rect, app: &mut App, config: &Config) {
    let slots = hotbar_panel_slots(app, config);
    let content_width = area.width.saturating_sub(4) as usize;
    // Title carries the modifier hint (⌥ on macOS, alt+ elsewhere) so users can
    // see *which* key to hold without it eating the tight 4-char slot cells —
    // it renders at every sidebar width and costs no slot-row height.
    let title = format!("Hotbar  {}1-8", super::widgets::key_hint::alt_prefix());
    render_sidebar_section(
        f,
        area,
        &title,
        hotbar_panel_lines(&slots, content_width, &app.ui_theme),
        hotbar_panel_hover_texts(&slots),
        Vec::new(),
        app,
    );
}

fn hotbar_panel_enabled(app: &App, config: &Config) -> bool {
    !resolved_hotbar_bindings(app, config).is_empty()
}

fn hotbar_panel_slots(app: &App, config: &Config) -> Vec<HotbarPanelSlot> {
    let mut bindings = resolved_hotbar_bindings(app, config)
        .into_iter()
        .map(|binding| (binding.slot, binding))
        .collect::<BTreeMap<_, _>>();

    // Lead each hover tip with the platform-correct chord (⌥+1 / alt+1); keep
    // the "Slot N" suffix so existing assertions and the "Slot" wording remain.
    let alt_prefix = super::widgets::key_hint::alt_prefix();
    (1..=codewhale_config::HOTBAR_SLOT_COUNT)
        .map(|slot| {
            let Some(binding) = bindings.remove(&slot) else {
                return HotbarPanelSlot {
                    slot,
                    label: "-".to_string(),
                    full_text: format!("{alt_prefix}{slot} · Slot {slot}: empty"),
                    state: HotbarSlotState::Empty,
                };
            };

            let Some(action) = app.hotbar_actions.get(&binding.action) else {
                let label = hotbar_configured_label(binding.label.as_deref())
                    .unwrap_or_else(|| "unknown".to_string());
                return HotbarPanelSlot {
                    slot,
                    label,
                    full_text: format!(
                        "{alt_prefix}{slot} · Slot {slot}: unknown action {}",
                        binding.action
                    ),
                    state: HotbarSlotState::Unknown,
                };
            };

            let label = hotbar_configured_label(binding.label.as_deref())
                .unwrap_or_else(|| action.short_label().to_string());
            let active = action.is_active(app);
            let state = if active {
                HotbarSlotState::Active
            } else {
                HotbarSlotState::Inactive
            };
            let status = if active { " active" } else { "" };
            HotbarPanelSlot {
                slot,
                label: label.clone(),
                full_text: format!(
                    "{alt_prefix}{slot} · Slot {slot}: {label}{status} ({}: {})",
                    action.category(),
                    action.id()
                ),
                state,
            }
        })
        .collect()
}

fn resolved_hotbar_bindings(app: &App, config: &Config) -> Vec<codewhale_config::HotbarBinding> {
    let known_action_ids = app
        .hotbar_actions
        .iter()
        .map(|action| action.id())
        .collect::<Vec<_>>();
    config.resolve_hotbar_bindings(&known_action_ids).bindings
}

fn hotbar_configured_label(label: Option<&str>) -> Option<String> {
    label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
}

fn hotbar_panel_lines(
    slots: &[HotbarPanelSlot],
    content_width: usize,
    theme: &palette::UiTheme,
) -> Vec<Line<'static>> {
    let gap_count = HOTBAR_ROW_COLUMNS.saturating_sub(1);
    let cell_width = content_width.saturating_sub(gap_count) / HOTBAR_ROW_COLUMNS;
    let cell_width = cell_width.max(1);

    slots
        .chunks(HOTBAR_ROW_COLUMNS)
        .map(|row| {
            let mut spans = Vec::with_capacity(row.len().saturating_mul(2));
            for (idx, slot) in row.iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::raw(" "));
                }
                spans.push(Span::styled(
                    hotbar_slot_cell_text(slot, cell_width),
                    hotbar_slot_style(slot, theme),
                ));
            }
            Line::from(spans)
        })
        .collect()
}

fn hotbar_panel_hover_texts(slots: &[HotbarPanelSlot]) -> Vec<String> {
    slots
        .chunks(HOTBAR_ROW_COLUMNS)
        .map(|row| {
            row.iter()
                .map(|slot| slot.full_text.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .collect()
}

fn hotbar_slot_cell_text(slot: &HotbarPanelSlot, cell_width: usize) -> String {
    let chord = format!("Alt{}", slot.slot);
    let marker = match slot.state {
        HotbarSlotState::Empty => "-",
        HotbarSlotState::Inactive => "",
        HotbarSlotState::Active => "*",
        HotbarSlotState::Unknown => "?",
    };
    let text = if marker.is_empty() {
        format!("{chord}:{}", slot.label)
    } else if slot.state == HotbarSlotState::Empty {
        format!("{chord}:{marker}")
    } else {
        format!("{chord}:{marker}{}", slot.label)
    };
    pad_to_display_width(clip_line_to_width(&text, cell_width), cell_width)
}

fn hotbar_slot_style(slot: &HotbarPanelSlot, theme: &palette::UiTheme) -> Style {
    match slot.state {
        HotbarSlotState::Empty => Style::default().fg(theme.text_dim),
        HotbarSlotState::Inactive => Style::default().fg(theme.text_body),
        HotbarSlotState::Active => Style::default()
            .fg(theme.accent_primary)
            .add_modifier(ratatui::style::Modifier::BOLD),
        HotbarSlotState::Unknown => Style::default().fg(theme.error_fg),
    }
}

fn clip_line_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out
}

fn pad_to_display_width(mut text: String, target_width: usize) -> String {
    let width = unicode_width::UnicodeWidthStr::width(text.as_str());
    if width < target_width {
        text.push_str(&" ".repeat(target_width - width));
    }
    text
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
    pause_indicator: Option<String>,
    workflow_paused: bool,
}

impl SidebarWorkSummary {
    fn checklist_is_primary(&self) -> bool {
        !self.checklist_items.is_empty()
    }

    fn checklist_is_complete(&self) -> bool {
        self.checklist_is_primary()
            && self
                .checklist_items
                .iter()
                .all(|item| item.status == TodoStatus::Completed)
    }

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

fn should_render_strategy_step(
    summary: &SidebarWorkSummary,
    step: &SidebarWorkStrategyStep,
) -> bool {
    !summary.checklist_is_complete() || step.status == StepStatus::Completed
}

fn renderable_strategy_steps(summary: &SidebarWorkSummary) -> Vec<&SidebarWorkStrategyStep> {
    summary
        .strategy_steps
        .iter()
        .filter(|step| should_render_strategy_step(summary, step))
        .collect()
}

fn has_renderable_strategy(summary: &SidebarWorkSummary) -> bool {
    summary
        .strategy_explanation
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
        || summary
            .strategy_steps
            .iter()
            .any(|step| should_render_strategy_step(summary, step))
}

fn sidebar_work_summary(app: &mut App) -> SidebarWorkSummary {
    fn live_goal_objective(app: &App) -> Option<String> {
        if app.paused || app.paused_quarry.is_some() {
            app.hunt
                .quarry
                .clone()
                .or_else(|| app.paused_quarry.clone())
        } else {
            app.hunt.quarry.clone()
        }
    }

    fn live_pause_indicator(app: &App) -> Option<String> {
        if app.paused && app.is_loading {
            Some("(Pausing)".to_string())
        } else if app.paused || app.paused_quarry.is_some() {
            Some("(Paused)".to_string())
        } else {
            None
        }
    }

    fn apply_live_goal_state(summary: &mut SidebarWorkSummary, app: &App) {
        summary.goal_objective = live_goal_objective(app);
        summary.goal_token_budget = app.hunt.token_budget;
        summary.goal_completed = app.hunt.verdict == HuntVerdict::Hunted;
        summary.goal_started_at = app.hunt.started_at;
        summary.tokens_used = app.session.total_conversation_tokens;
        summary.pause_indicator = live_pause_indicator(app);
        summary.workflow_paused = app.paused || app.paused_quarry.is_some();
    }

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

        let mut summary = SidebarWorkSummary {
            goal_objective: live_goal_objective(app),
            goal_token_budget: app.hunt.token_budget,
            goal_completed: app.hunt.verdict == HuntVerdict::Hunted,
            goal_started_at: app.hunt.started_at,
            tokens_used: app.session.total_conversation_tokens,
            checklist_completion_pct,
            checklist_items,
            strategy_explanation,
            strategy_steps,
            state_updating: false,
            pause_indicator: live_pause_indicator(app),
            workflow_paused: app.paused || app.paused_quarry.is_some(),
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

fn work_panel_hover_texts(
    summary: &SidebarWorkSummary,
    content_width: usize,
    max_rows: usize,
) -> Vec<String> {
    let mut texts = Vec::with_capacity(max_rows.max(4));

    if let Some(objective) = summary.goal_objective.as_deref()
        && !objective.trim().is_empty()
        && texts.len() < max_rows
    {
        let icon = if summary.goal_completed {
            "✓"
        } else if summary.workflow_paused {
            "⏸"
        } else {
            "◆"
        };
        texts.push(format!("{icon} {objective}"));

        if let Some(started) = summary.goal_started_at
            && texts.len() < max_rows
        {
            let elapsed = crate::tui::notifications::humanize_duration(started.elapsed());
            let elapsed_str = if summary.goal_completed {
                format!("completed in {elapsed}")
            } else {
                format!("elapsed: {elapsed}")
            };
            texts.push(elapsed_str);
        }

        if let Some(budget) = summary.goal_token_budget
            && texts.len() < max_rows
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
            texts.push(format!(
                "tokens: {}/{} {}",
                summary.tokens_used, budget, bar
            ));
        }
    }

    if summary.state_updating && texts.len() < max_rows {
        texts.push("Work state updating...".to_string());
    }

    if !summary.checklist_items.is_empty() && texts.len() < max_rows {
        let total = summary.checklist_items.len();
        let completed = summary
            .checklist_items
            .iter()
            .filter(|item| item.status == TodoStatus::Completed)
            .count();
        texts.push(format!(
            "{}% complete ({completed}/{total})",
            summary.checklist_completion_pct
        ));

        let reserve_for_strategy = if has_renderable_strategy(summary) {
            2
        } else {
            0
        };
        let available_item_rows = max_rows
            .saturating_sub(texts.len())
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
            let prefix = match item.status {
                TodoStatus::Pending => "[ ]",
                TodoStatus::InProgress => "[~]",
                TodoStatus::Completed => "[✓]",
            };
            texts.push(format!("{prefix} #{} {}", item.id, item.content));
        }

        let earlier = start;
        let later = summary.checklist_items.len().saturating_sub(end);
        let remaining = earlier.saturating_add(later);
        if remaining > 0 && texts.len() < max_rows {
            let mut label = match (earlier, later) {
                (0, later) => format!("+{later} more checklist items"),
                (earlier, 0) => format!("+{earlier} earlier checklist items"),
                (earlier, later) => format!("+{earlier} earlier, +{later} later"),
            };
            // Hovering the overflow row reveals the omitted items, since
            // the compact panel gives no other way to inspect them (#3063).
            let omitted = summary.checklist_items[..start]
                .iter()
                .chain(summary.checklist_items[end..].iter());
            for item in omitted {
                let prefix = match item.status {
                    TodoStatus::Pending => "[ ]",
                    TodoStatus::InProgress => "[~]",
                    TodoStatus::Completed => "[✓]",
                };
                let _ = write!(label, "\n{prefix} #{} {}", item.id, item.content);
            }
            texts.push(label);
        }
    }

    if has_renderable_strategy(summary) && texts.len() < max_rows {
        let strategy_steps = renderable_strategy_steps(summary);

        if !summary.checklist_is_primary() && !summary.strategy_steps.is_empty() {
            let (pending, in_progress, completed) = summary.strategy_counts();
            let total = pending + in_progress + completed;
            texts.push(format!(
                "Strategy metadata {}% complete ({completed}/{total})",
                summary.strategy_progress_percent()
            ));
        } else {
            texts.push(work_strategy_context_label(summary).to_string());
        }

        if let Some(explanation) = summary.strategy_explanation.as_deref()
            && texts.len() < max_rows
        {
            texts.push(explanation.to_string());
        }

        let max_steps = max_rows
            .saturating_sub(texts.len())
            .min(strategy_steps.len());
        let remaining = strategy_steps.len().saturating_sub(max_steps);
        for step in strategy_steps.into_iter().take(max_steps) {
            let prefix = match step.status {
                StepStatus::Pending => "[ ]",
                StepStatus::InProgress => "[~]",
                StepStatus::Completed => "[✓]",
            };
            let mut text = if summary.checklist_is_primary() {
                format!(
                    "{} {}",
                    strategy_context_step_prefix(&step.status),
                    step.text
                )
            } else {
                format!("{prefix} {}", step.text)
            };
            if !step.elapsed.is_empty() {
                let _ = write!(text, " ({})", step.elapsed);
            }
            texts.push(text);
        }

        if remaining > 0 && texts.len() < max_rows {
            texts.push(format!("+{remaining} more strategy steps"));
        }
    }

    if texts.is_empty() {
        texts.push("No active work".to_string());
    }

    texts
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
    } else if summary.workflow_paused {
        "⏸"
    } else {
        "◆"
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
    let label = if let Some(indicator) = summary.pause_indicator.as_deref() {
        format!("{objective} {indicator}")
    } else {
        objective.to_string()
    };

    lines.push(Line::from(Span::styled(
        format!(
            "{} {}",
            icon,
            truncate_line_to_width(&label, content_width.saturating_sub(2).max(1))
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

    let reserve_for_strategy = if has_renderable_strategy(summary) {
        2
    } else {
        0
    };
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
    if !has_renderable_strategy(summary) || lines.len() >= max_rows {
        return;
    }

    let checklist_is_primary = summary.checklist_is_primary();
    let strategy_steps = renderable_strategy_steps(summary);
    if !checklist_is_primary && !summary.strategy_steps.is_empty() {
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
            work_strategy_context_label(summary),
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
        .min(strategy_steps.len());
    let remaining = strategy_steps.len().saturating_sub(max_steps);
    for step in strategy_steps.into_iter().take(max_steps) {
        let (prefix, color) = match step.status {
            StepStatus::Pending => ("[ ]", theme.plan_pending_color),
            StepStatus::InProgress => ("[~]", theme.plan_in_progress_color),
            StepStatus::Completed => ("[✓]", theme.plan_completed_color),
        };
        let (text_prefix, color) = if checklist_is_primary {
            (
                strategy_context_step_prefix(&step.status),
                strategy_context_step_color(&step.status, theme),
            )
        } else {
            (prefix, color)
        };
        let mut text = format!("{text_prefix} {}", step.text);
        if !step.elapsed.is_empty() {
            let _ = write!(text, " ({})", step.elapsed);
        }
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&text, content_width),
            Style::default().fg(color),
        )));
    }

    if remaining > 0 && lines.len() < max_rows {
        lines.push(Line::from(Span::styled(
            format!("+{remaining} more strategy steps"),
            Style::default().fg(theme.plan_summary_color),
        )));
    }
}

fn work_strategy_context_label(summary: &SidebarWorkSummary) -> &'static str {
    if summary.checklist_is_primary() {
        "Strategy context"
    } else {
        "Strategy metadata"
    }
}

fn strategy_context_step_prefix(status: &StepStatus) -> &'static str {
    match status {
        StepStatus::Pending => "phase next:",
        StepStatus::InProgress => "phase now:",
        StepStatus::Completed => "phase done:",
    }
}

fn strategy_context_step_color(status: &StepStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        StepStatus::Pending => theme.plan_pending_color,
        StepStatus::InProgress => theme.plan_in_progress_color,
        StepStatus::Completed => theme.plan_summary_color,
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

    let full_texts = work_panel_hover_texts(&summary, content_width.max(1), usable_rows);
    render_sidebar_section(f, area, "To-do", lines, full_texts, Vec::new(), app);
}

/// Click actions for one background job row pair (#3028).
///
/// Returns `(show, detail)` where `show` opens the job and `detail` cancels
/// it while it is still running (finished jobs make the detail row a second
/// show target instead — cancel would only error). `shell_*` ids belong to
/// the shell job manager and route through `/jobs`; everything else routes
/// through `/task`.
fn background_task_click_actions(task: &TaskPanelEntry) -> (String, String) {
    let namespace = if task.id.starts_with("shell_") {
        "jobs"
    } else {
        "task"
    };
    let show = format!("/{namespace} show {}", task.id);
    let detail = if matches!(task.status.as_str(), "running" | "queued") {
        format!("/{namespace} cancel {}", task.id)
    } else {
        show.clone()
    };
    (show, detail)
}

fn background_task_has_stop_target(task: &TaskPanelEntry) -> bool {
    matches!(task.status.as_str(), "running" | "queued")
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

fn render_sidebar_tasks(f: &mut Frame, area: Rect, app: &mut App) {
    if area.height < 3 {
        return;
    }

    let content_width = area.width.saturating_sub(4) as usize;
    let usable_rows = area.height.saturating_sub(3) as usize;
    let row_sets = task_panel_row_sets(app);
    let (lines, row_actions) =
        task_panel_rows(app, &row_sets, content_width.max(1), usable_rows.max(1));

    let full_texts = task_panel_hover_texts(app, &row_sets, usable_rows.max(1));
    render_sidebar_section(f, area, "Tasks", lines, full_texts, row_actions, app);
}

#[derive(Debug, Clone)]
struct SidebarToolRow {
    name: String,
    status: ToolStatus,
    summary: String,
    duration_ms: Option<u64>,
}

/// Row sets shared by the Tasks panel line renderer and hover-text builder.
///
/// Computed once per frame in `render_sidebar_tasks` (#3898): both consumers
/// previously recomputed `active_tool_rows` / `reasoning_task_rows` /
/// `background_task_rows` (a clone+sort of `app.task_panel`) independently,
/// doubling the work and risking the two passes disagreeing across an
/// `Instant::elapsed` TTL boundary within the same frame.
struct TaskPanelRowSets {
    active: Vec<SidebarToolRow>,
    reasoning: Vec<TaskPanelEntry>,
    background: Vec<TaskPanelEntry>,
    recent: Vec<SidebarToolRow>,
}

fn task_panel_row_sets(app: &App) -> TaskPanelRowSets {
    let explicit_tasks_focus = app.sidebar_focus == SidebarFocus::Tasks;
    let active = active_tool_rows(app);
    let reasoning = reasoning_task_rows(app);
    // Auto/Pinned mode deliberately skips live-tool dedup (passes an empty
    // slice), matching the previous per-consumer call sites.
    let background = background_task_rows(app, if explicit_tasks_focus { &active } else { &[] });
    let recent = if explicit_tasks_focus {
        recent_tool_rows(app, 4)
    } else {
        Vec::new()
    };
    TaskPanelRowSets {
        active,
        reasoning,
        background,
        recent,
    }
}

#[cfg(test)]
fn task_panel_lines(app: &App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    task_panel_rows(app, &task_panel_row_sets(app), content_width, max_rows).0
}

/// Build the Tasks panel lines together with a parallel per-line click-action
/// vector (#3028). Producing both in a single pass keeps the action indices
/// aligned with the rendered lines no matter how the layout evolves.
fn task_panel_rows(
    app: &App,
    row_sets: &TaskPanelRowSets,
    content_width: usize,
    max_rows: usize,
) -> (Vec<Line<'static>>, Vec<Option<String>>) {
    let theme = &app.ui_theme;
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));
    let mut actions: Vec<Option<String>> = Vec::with_capacity(max_rows.max(4));
    let explicit_tasks_focus = app.sidebar_focus == SidebarFocus::Tasks;

    if explicit_tasks_focus && app.runtime_turn_id.is_some() {
        let status = app
            .runtime_turn_status
            .as_deref()
            .unwrap_or("unknown")
            .to_string();
        // #3030: Use a stable turn number ("Turn 1") instead of the raw
        // UUID prefix.  The full UUID is preserved in the hover text
        // (task_panel_hover_texts) for inspection.
        let turn_label = if app.turn_counter > 0 {
            format!("Turn {} ({status})", app.turn_counter)
        } else {
            format!("Current turn ({status})")
        };
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&turn_label, content_width.max(1)),
            Style::default().fg(theme.accent_primary),
        )));
    }

    let active_rows = &row_sets.active;
    if explicit_tasks_focus && !active_rows.is_empty() && lines.len() < max_rows {
        push_sidebar_label_theme(&mut lines, "Live tools", theme);
        push_tool_rows(&mut lines, active_rows, content_width, max_rows, theme);
    }

    let reasoning_rows = &row_sets.reasoning;
    if explicit_tasks_focus && !reasoning_rows.is_empty() && lines.len() < max_rows {
        push_sidebar_label_theme(&mut lines, "Model reasoning", theme);
        push_reasoning_rows(&mut lines, reasoning_rows, content_width, max_rows, theme);
    }

    let background_rows = &row_sets.background;
    // Lines pushed so far (turn label, Live tools header, live tool rows)
    // are not clickable — backfill their action slots.
    actions.resize(lines.len(), None);
    if !background_rows.is_empty() && lines.len() < max_rows {
        let running = background_rows
            .iter()
            .filter(|task| task.status == "running")
            .count();
        let done = background_rows.len().saturating_sub(running);
        let label = if running == 0 {
            format!("Bash jobs: {done} completed")
        } else if done == 0 {
            format!("Bash jobs: {running} running")
        } else {
            format!("Bash jobs: {running} running, {done} completed")
        };
        lines.push(Line::from(Span::styled(
            label,
            Style::default().fg(theme.accent_primary).bold(),
        )));
        actions.push(None);

        let max_items = max_rows.saturating_sub(lines.len());
        for task in background_rows.iter().take(max_items) {
            let color = if task.stale && task.status == "running" {
                theme.warning
            } else {
                match task.status.as_str() {
                    "queued" => theme.text_muted,
                    "running" => theme.warning,
                    "completed" => theme.success,
                    "failed" => theme.error_fg,
                    "canceled" => theme.text_dim,
                    _ => theme.text_muted,
                }
            };
            let duration = task
                .duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string());
            let (label, detail) = background_task_labels(task, &duration);
            let label = background_task_spinner_prefix(task)
                .map(|prefix| format!("{prefix} {label}"))
                .unwrap_or(label);
            let (show_action, detail_action) = background_task_click_actions(task);
            let label = if background_task_has_stop_target(task) {
                label_with_stop_target(&label, content_width.max(1))
            } else {
                truncate_line_to_width(&label, content_width.max(1))
            };
            lines.push(Line::from(Span::styled(label, Style::default().fg(color))));
            actions.push(Some(show_action));
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_line_to_width(&detail, content_width.saturating_sub(2).max(1))
                ),
                Style::default().fg(theme.text_dim),
            )));
            actions.push(Some(detail_action));
        }

        if lines.len() < max_rows {
            let stale_running_shells = background_rows
                .iter()
                .filter(|task| {
                    task.id.starts_with("shell_") && task.status == "running" && task.stale
                })
                .collect::<Vec<_>>();
            let any_running_shell = background_rows
                .iter()
                .any(|task| task.id.starts_with("shell_") && task.status == "running");
            let hint_action = if stale_running_shells.len() == 1 {
                Some((
                    "Ctrl+X -> cancel stale job".to_string(),
                    format!("/jobs cancel {}", stale_running_shells[0].id),
                ))
            } else if any_running_shell {
                Some((
                    "Ctrl+X -> /jobs cancel-all".to_string(),
                    "/jobs cancel-all".to_string(),
                ))
            } else {
                None
            };
            if let Some((hint, action)) = hint_action {
                lines.push(Line::from(Span::styled(
                    truncate_line_to_width(&hint, content_width.max(1)),
                    Style::default()
                        .fg(theme.text_muted)
                        .add_modifier(ratatui::style::Modifier::ITALIC),
                )));
                actions.push(Some(action));
            }
        }
    }

    if explicit_tasks_focus && lines.len() < max_rows {
        let recent_rows = &row_sets.recent;
        if !recent_rows.is_empty() {
            push_sidebar_label_theme(&mut lines, "Recent tools", theme);
            push_tool_rows(&mut lines, recent_rows, content_width, max_rows, theme);
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
            && reasoning_rows.is_empty()
            && background_rows.is_empty())
    {
        lines.push(Line::from(Span::styled(
            "No live tools or background jobs",
            Style::default().fg(theme.text_muted),
        )));
    }

    // Backfill action slots for the trailing non-clickable lines (Recent
    // tools, yank hint, empty-state notice).
    actions.resize(lines.len(), None);
    (lines, actions)
}

fn task_panel_hover_texts(app: &App, row_sets: &TaskPanelRowSets, max_rows: usize) -> Vec<String> {
    let mut texts = Vec::with_capacity(max_rows.max(4));
    let explicit_tasks_focus = app.sidebar_focus == SidebarFocus::Tasks;

    if explicit_tasks_focus && let Some(turn_id) = app.runtime_turn_id.as_ref() {
        let status = app.runtime_turn_status.as_deref().unwrap_or("unknown");
        texts.push(format!("turn {turn_id} ({status})"));
    }

    let active_rows = &row_sets.active;
    if explicit_tasks_focus && !active_rows.is_empty() && texts.len() < max_rows {
        texts.push("Live tools".to_string());
        push_tool_row_hover_texts(&mut texts, active_rows, max_rows);
    }

    let reasoning_rows = &row_sets.reasoning;
    if explicit_tasks_focus && !reasoning_rows.is_empty() && texts.len() < max_rows {
        texts.push("Model reasoning".to_string());
        push_reasoning_row_hover_texts(&mut texts, reasoning_rows, max_rows);
    }

    let background_rows = &row_sets.background;
    if !background_rows.is_empty() && texts.len() < max_rows {
        let running = background_rows
            .iter()
            .filter(|task| task.status == "running")
            .count();
        let done = background_rows.len().saturating_sub(running);
        let label = if running == 0 {
            format!("Bash jobs: {done} completed")
        } else if done == 0 {
            format!("Bash jobs: {running} running")
        } else {
            format!("Bash jobs: {running} running, {done} completed")
        };
        texts.push(label);

        let max_items = max_rows.saturating_sub(texts.len());
        for task in background_rows.iter().take(max_items) {
            let duration = task
                .duration_ms
                .map(format_duration_ms)
                .unwrap_or_else(|| "-".to_string());
            let (label, detail) = background_task_labels(task, &duration);
            let label = background_task_spinner_prefix(task)
                .map(|prefix| format!("{prefix} {label}"))
                .unwrap_or(label);
            texts.push(label);
            if texts.len() >= max_rows {
                break;
            }
            texts.push(format!("  {detail}"));
        }

        if texts.len() < max_rows {
            let stale_running_shells = background_rows
                .iter()
                .filter(|task| {
                    task.id.starts_with("shell_") && task.status == "running" && task.stale
                })
                .count();
            let any_running_shell = background_rows
                .iter()
                .any(|task| task.id.starts_with("shell_") && task.status == "running");
            if stale_running_shells == 1 {
                texts.push("Ctrl+X -> cancel stale job".to_string());
            } else if any_running_shell {
                texts.push("Ctrl+X -> /jobs cancel-all".to_string());
            }
        }
    }

    if explicit_tasks_focus && texts.len() < max_rows {
        let recent_rows = &row_sets.recent;
        if !recent_rows.is_empty() {
            texts.push("Recent tools".to_string());
            push_tool_row_hover_texts(&mut texts, recent_rows, max_rows);
        }
    }

    if texts.len() + 1 < max_rows
        && app.runtime_turn_id.is_some()
        && app.sidebar_focus == SidebarFocus::Tasks
    {
        texts.push("y -> copy turn id  ·  Y -> copy full status".to_string());
    }

    if texts.is_empty()
        || (texts.len() == 1
            && app.runtime_turn_id.is_some()
            && active_rows.is_empty()
            && reasoning_rows.is_empty()
            && background_rows.is_empty())
    {
        texts.push("No live tools or background jobs".to_string());
    }

    texts
}

fn push_sidebar_label_theme(lines: &mut Vec<Line<'static>>, label: &str, theme: &palette::UiTheme) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default().fg(theme.accent_primary).bold(),
    )));
}

fn push_tool_row_hover_texts(texts: &mut Vec<String>, rows: &[SidebarToolRow], max_rows: usize) {
    for row in rows {
        if texts.len() >= max_rows {
            break;
        }
        let (marker, _) = tool_status_marker(row.status, &palette::UI_THEME);
        let label = if let Some(duration_ms) = row.duration_ms {
            format!("{marker} {} {}", row.name, format_duration_ms(duration_ms))
        } else {
            format!("{marker} {}", row.name)
        };
        texts.push(label);
        if !row.summary.trim().is_empty() && texts.len() < max_rows {
            texts.push(format!("  {}", row.summary));
        }
    }
}

fn push_reasoning_rows(
    lines: &mut Vec<Line<'static>>,
    rows: &[TaskPanelEntry],
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) {
    for task in rows {
        if lines.len() >= max_rows {
            break;
        }
        let color = match task.status.as_str() {
            "running" => theme.warning,
            "completed" => theme.success,
            "failed" => theme.error_fg,
            _ => theme.text_muted,
        };
        let duration = task
            .duration_ms
            .map(format_duration_ms)
            .unwrap_or_else(|| "-".to_string());
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(
                &format!("thinking {} {duration}", task.status),
                content_width,
            ),
            Style::default().fg(color),
        )));
        if !task.prompt_summary.trim().is_empty() && lines.len() < max_rows {
            lines.push(Line::from(Span::styled(
                format!(
                    "  {}",
                    truncate_line_to_width(
                        &task.prompt_summary,
                        content_width.saturating_sub(2).max(1)
                    )
                ),
                Style::default().fg(theme.text_dim),
            )));
        }
    }
}

fn push_reasoning_row_hover_texts(
    texts: &mut Vec<String>,
    rows: &[TaskPanelEntry],
    max_rows: usize,
) {
    for task in rows {
        if texts.len() >= max_rows {
            break;
        }
        let duration = task
            .duration_ms
            .map(format_duration_ms)
            .unwrap_or_else(|| "-".to_string());
        texts.push(format!("thinking {} {duration}", task.status));
        if !task.prompt_summary.trim().is_empty() && texts.len() < max_rows {
            texts.push(format!("  {}", task.prompt_summary));
        }
    }
}

fn background_task_labels(task: &TaskPanelEntry, duration: &str) -> (String, String) {
    let stale_label = stale_no_output_label(task);
    let owner_label = task
        .owner_agent_name
        .as_deref()
        .or(task.owner_agent_id.as_deref())
        .filter(|owner| !owner.trim().is_empty())
        .map(|owner| format!("by {owner}"))
        .unwrap_or_default();
    let status = stale_label
        .as_ref()
        .map(|label| format!("{} ({label})", task.status))
        .unwrap_or_else(|| task.status.clone());

    if let Some(command) = task.prompt_summary.strip_prefix("shell: ") {
        let command = concise_shell_command_label(command, 96);
        return (
            format!("Bash {status} {command} {duration}"),
            compact_join([
                format!("{} \u{00B7} Bash", task.id),
                owner_label,
                stale_label.unwrap_or_default(),
            ]),
        );
    }

    (
        format!(
            "{} {} {}",
            truncate_line_to_width(&task.id, 10),
            status,
            duration
        ),
        compact_join([
            task.prompt_summary.clone(),
            owner_label,
            stale_label.unwrap_or_default(),
        ]),
    )
}

fn background_task_is_live(task: &TaskPanelEntry) -> bool {
    task.kind == TaskPanelEntryKind::Background
        && matches!(task.status.as_str(), "queued" | "running")
}

fn background_task_spinner_prefix(task: &TaskPanelEntry) -> Option<&'static str> {
    if task.status != "running" {
        return None;
    }
    Some(braille_spinner_frame_for_duration_ms(
        task.duration_ms.unwrap_or_default(),
        false,
    ))
}

fn stale_no_output_label(task: &TaskPanelEntry) -> Option<String> {
    if !(task.stale && task.status == "running") {
        return None;
    }
    task.elapsed_since_output_ms
        .map(format_duration_ms)
        .map(|duration| format!("stale, no output {duration}"))
        .or_else(|| Some("stale, no output".to_string()))
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
                .snapshot
                .objective
                .as_deref()
                .or(plan.snapshot.title.as_deref())
                .or(plan.snapshot.explanation.as_deref())
                .or(plan.snapshot.recommended_approach.as_deref())
                .or_else(|| plan.snapshot.items.first().map(|step| step.step.as_str()))
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
            "Waiting for CI \u{00B7} {}",
            crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details")
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
    let hint = crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details");
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
        "task_shell_start" => "start Bash",
        "task_shell_wait" => "wait Bash",
        "task_shell_write" => "write Bash",
        _ => name,
    }
}

fn generic_tool_sidebar_summary(generic: &GenericToolCell) -> String {
    match generic.name.as_str() {
        "task_shell_start" => compact_join([
            generic.input_summary.clone().unwrap_or_default(),
            "background Bash".to_string(),
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
        .filter(|task| task.kind == TaskPanelEntryKind::Background)
        .filter(|task| !background_task_duplicates_live_tool(task, active_rows))
        .cloned()
        .collect();
    rows.sort_by_key(|task| (task_status_rank(task.status.as_str()), task.id.clone()));
    rows
}

fn reasoning_task_rows(app: &App) -> Vec<TaskPanelEntry> {
    let mut rows: Vec<TaskPanelEntry> = app
        .task_panel
        .iter()
        .filter(|task| task.kind == TaskPanelEntryKind::ModelReasoning)
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
                "{command} \u{00B7} {count} polls collapsed \u{00B7} {}",
                crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details")
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
    row.status == ToolStatus::Running
        && matches!(row.name.as_str(), "wait Bash" | "exec_shell_wait")
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

    normalize_activity_text(&row.name)
}

fn normalize_activity_text(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tool_row_rank(row: &SidebarToolRow) -> u8 {
    match row.status {
        ToolStatus::Failed => 0,
        // A schema-hydrated deferred tool is not "run done" — it must be
        // retried — so it ranks with active work, not completed successes.
        ToolStatus::Running | ToolStatus::Hydrated => 1,
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
        ToolStatus::Hydrated => ("[~]", theme.warning),
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
    let (lines, row_actions) = subagent_panel_rows(
        &summary,
        &rows,
        content_width,
        usable_rows.max(1),
        &app.ui_theme,
    );
    let full_texts = subagent_panel_hover_texts(&summary, &rows, usable_rows.max(1));

    render_sidebar_section(f, area, "Agents", lines, full_texts, row_actions, app);
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
    pub parent_run_id: Option<String>,
    pub spawn_depth: u32,
    pub name: String,
    pub role: String,
    pub status: String,
    pub objective: Option<String>,
    pub git_branch: Option<String>,
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
            // #3030: Prefer the user-assigned nickname > stable label
            // ("Agent 1") > raw name. Every spawned agent gets a label-map
            // entry, so the generated label must not shadow nicknames.
            let display_name = agent
                .nickname
                .clone()
                .or_else(|| app.agent_label_map.get(&agent.agent_id).cloned())
                .unwrap_or_else(|| agent.name.clone());
            SidebarAgentRow {
                id: agent.agent_id.clone(),
                parent_run_id: agent.parent_run_id.clone(),
                spawn_depth: agent.spawn_depth,
                name: display_name,
                role: agent.agent_type.as_str().to_string(),
                status: agent
                    .worker_status
                    .map(sidebar_worker_status_text)
                    .unwrap_or_else(|| subagent_status_text(&agent.status))
                    .to_string(),
                objective: Some(agent.assignment.objective.clone())
                    .filter(|objective| !objective.trim().is_empty()),
                git_branch: agent.git_branch.clone(),
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
            .map(|(id, progress)| {
                // #3030: Prefer stable label for progress-only agents too.
                let display_name = app
                    .agent_label_map
                    .get(id.as_str())
                    .cloned()
                    .unwrap_or_else(|| id.clone());
                let meta = app.agent_progress_meta.get(id.as_str());
                let spawn_depth = meta.map(|meta| meta.spawn_depth).unwrap_or_default();
                SidebarAgentRow {
                    id: id.clone(),
                    parent_run_id: meta.and_then(|meta| meta.parent_run_id.clone()),
                    spawn_depth,
                    name: display_name,
                    role: if spawn_depth > 1 {
                        "child".to_string()
                    } else {
                        "agent".to_string()
                    },
                    status: sidebar_progress_status_text(progress).to_string(),
                    objective: None,
                    git_branch: None,
                    progress: Some(progress.clone()),
                    steps_taken: 0,
                    duration_ms: None,
                }
            }),
    );

    sort_sidebar_agent_rows_as_tree(rows)
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

fn sidebar_progress_status_text(progress: &str) -> &'static str {
    let lower = progress.to_ascii_lowercase();
    if lower.contains("queued") {
        "queued"
    } else if lower.contains("waiting for user") || lower.contains("waiting for follow-up") {
        "waiting"
    } else if lower.contains("waiting for model") || lower.contains("requesting model") {
        "model wait"
    } else if lower.contains("running tool")
        || lower.contains("executing tool")
        || lower.contains("tool:")
    {
        "tool"
    } else if lower.contains("starting") {
        "starting"
    } else {
        agent_worker_status_name(AgentWorkerStatus::Running)
    }
}

/// Build sub-agent sidebar lines from summary + per-agent rows. Public
/// for the snapshot tests in this module.
#[cfg(test)]
pub fn subagent_panel_lines(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) -> Vec<Line<'static>> {
    subagent_panel_rows(summary, rows, content_width, max_rows, theme).0
}

/// Build the Agents panel lines together with a parallel per-line
/// click-action vector (#3028). Agent label rows open the Fleet worker status
/// view via `/fleet status`; header, role-mix, detail, and RLM lines are not
/// clickable.
fn subagent_panel_rows(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    content_width: usize,
    max_rows: usize,
    theme: &palette::UiTheme,
) -> (Vec<Line<'static>>, Vec<Option<String>>) {
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(max_rows.max(4));
    let mut actions: Vec<Option<String>> = Vec::with_capacity(max_rows.max(4));

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
    lines.push(Line::from(header));
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
        let label = format!("{tree_prefix}{marker} {} {}", row.role, row.name);
        lines.push(Line::from(Span::styled(
            truncate_line_to_width(&label, content_width.max(1)),
            Style::default().fg(color),
        )));
        actions.push(Some("/fleet status".to_string()));

        // Auto-collapse finished sub-agents so the sidebar stays compact when
        // work is done or terminally stopped.
        if sidebar_agent_status_is_terminal(row.status.as_str()) {
            continue;
        }

        if lines.len() >= max_rows {
            break;
        }
        // #3030: keep raw agent ids out of the compact detail line — the
        // full id remains available in the hover text.
        let mut detail_parts = Vec::new();
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
        if detail_parts.is_empty() {
            detail_parts.push(row.status.clone());
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
        actions.push(None);
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

fn subagent_panel_hover_texts(
    summary: &SidebarSubagentSummary,
    rows: &[SidebarAgentRow],
    max_rows: usize,
) -> Vec<String> {
    let mut texts = Vec::with_capacity(max_rows.max(4));

    let fanout_total = summary.fanout_total.unwrap_or(0);
    if summary.cached_total == 0
        && summary.progress_only_count == 0
        && fanout_total == 0
        && !summary.foreground_rlm_running
    {
        texts.push("No agents".to_string());
        return texts;
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
    if live_running > 0 {
        texts.push(format!("{live_running} running / {total}"));
    } else {
        texts.push(format!("{done} done"));
    }

    if !summary.role_counts.is_empty() && texts.len() < max_rows {
        let mix: Vec<String> = summary
            .role_counts
            .iter()
            .map(|(role, count)| format!("{count} {role}"))
            .collect();
        texts.push(mix.join(" · "));
    }

    for row in rows {
        if texts.len() >= max_rows {
            break;
        }
        // The compact label row truncates aggressively, so its hover text
        // carries the full agent dossier: id, role, status, elapsed,
        // objective, branch, and untruncated progress (#3063).
        texts.push(agent_row_hover_text(row));

        if row.status == "done" {
            continue;
        }

        if texts.len() >= max_rows {
            break;
        }
        let mut detail_parts = Vec::new();
        detail_parts.push(row.id.clone());
        if row.steps_taken > 0 {
            detail_parts.push(format!("{} step(s)", row.steps_taken));
        }
        if let Some(progress) = row.progress.as_deref()
            && !progress.trim().is_empty()
        {
            detail_parts.push(progress.trim().to_string());
        }
        if let Some(branch) = row.git_branch.as_deref() {
            detail_parts.push(format!("branch {branch}"));
        }
        if let Some(duration) = row.duration_ms {
            detail_parts.push(format_duration_ms(duration));
        }
        texts.push(format!("  {}", detail_parts.join(" · ")));
    }

    if summary.foreground_rlm_running && texts.len() < max_rows {
        texts.push("RLM foreground work active".to_string());
    }

    texts
}

/// Full hover dossier for one Agents-panel label row (#3063). The compact
/// row only shows `marker role name`, so hovering reveals everything else
/// without spamming raw ids into the normal view.
fn agent_row_hover_text(row: &SidebarAgentRow) -> String {
    let (marker, _) = agent_status_marker(row.status.as_str(), &palette::UI_THEME);
    let mut text = format!(
        "{}{} {} {}",
        agent_tree_prefix(row),
        marker,
        row.role,
        row.name
    );
    let _ = write!(text, "\nid: {}", row.id);
    if let Some(parent) = row.parent_run_id.as_deref() {
        let _ = write!(text, "\nparent: {parent}");
    }
    if row.spawn_depth > 0 {
        let _ = write!(text, "\ndepth: {}", row.spawn_depth);
    }
    let mut status_line = format!("status: {}", row.status);
    if let Some(duration) = row.duration_ms {
        let _ = write!(status_line, " · elapsed {}", format_duration_ms(duration));
    }
    if row.steps_taken > 0 {
        let _ = write!(status_line, " · {} step(s)", row.steps_taken);
    }
    let _ = write!(text, "\n{status_line}");
    if let Some(objective) = row.objective.as_deref() {
        let _ = write!(text, "\nobjective: {}", objective.trim());
    }
    if let Some(branch) = row.git_branch.as_deref() {
        let _ = write!(text, "\nbranch: {branch}");
    }
    if let Some(progress) = row.progress.as_deref()
        && !progress.trim().is_empty()
    {
        let _ = write!(text, "\nprogress: {}", progress.trim());
    }
    text
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
    let window = crate::route_budget::route_context_window_tokens(
        app.api_provider,
        app.effective_model_for_budget(),
        app.active_route_limits,
    );
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
    let cost_line = context_panel_cost_line(app);
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

    render_sidebar_section(f, area, "Session", lines, Vec::new(), Vec::new(), app);
}

fn context_panel_cost_line(app: &App) -> String {
    let displayed_total = app.displayed_session_cost_for_currency(app.cost_currency);
    if displayed_total == 0.0 && !crate::pricing::has_pricing_for_model(&app.model) {
        return format!("cost: n/a (no pricing data for {})", app.model);
    }

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
        format!("cost: {}", app.format_cost_amount(displayed_total))
    }
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
    row_actions: Vec<Option<String>>,
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
    let display_texts: Vec<String> = lines
        .iter()
        .map(|line| spans_to_text(&line.spans))
        .collect();
    let hover_texts: Vec<String> = display_texts
        .iter()
        .enumerate()
        .map(|(idx, display)| {
            full_texts
                .get(idx)
                .filter(|text| !text.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| display.clone())
        })
        .collect();
    let rows = sidebar_hover_rows(content_area, &display_texts, &hover_texts, &row_actions);
    app.sidebar_hover.sections.push(SidebarHoverSection {
        content_area,
        lines: hover_texts,
        rows,
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

fn sidebar_hover_rows(
    content_area: Rect,
    display_texts: &[String],
    hover_texts: &[String],
    row_actions: &[Option<String>],
) -> Vec<SidebarHoverRow> {
    display_texts
        .iter()
        .zip(hover_texts.iter())
        .enumerate()
        .map(|(idx, (display_text, full_text))| {
            let row_y = content_area.y.saturating_add(idx as u16);
            let display_width = unicode_width::UnicodeWidthStr::width(display_text.as_str());
            let full_width = unicode_width::UnicodeWidthStr::width(full_text.as_str());
            let click_action = row_actions.get(idx).and_then(|a| a.clone());
            let stop_action = display_text
                .ends_with(TASK_STOP_TARGET_LABEL)
                .then(|| row_actions.get(idx + 1).and_then(|a| a.clone()))
                .flatten()
                .filter(|action| action.contains(" cancel "));
            let stop_target_width = unicode_width::UnicodeWidthStr::width(TASK_STOP_TARGET_LABEL);
            let (stop_zone_start_col, stop_zone_end_col) =
                if stop_action.is_some() && display_width >= stop_target_width {
                    let visible_width = display_width.min(content_area.width as usize);
                    let start = content_area.x.saturating_add(
                        visible_width
                            .saturating_sub(stop_target_width)
                            .min(u16::MAX as usize) as u16,
                    );
                    let end = start.saturating_add(stop_target_width as u16);
                    (Some(start), Some(end))
                } else {
                    (None, None)
                };
            SidebarHoverRow {
                row_y,
                display_text: display_text.clone(),
                full_text: full_text.clone(),
                detail: None,
                is_truncated: display_width > content_area.width as usize
                    || full_width > content_area.width as usize
                    || display_text != full_text,
                click_action,
                stop_action,
                stop_zone_start_col,
                stop_zone_end_col,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_TOOL_COMPLETED_ROW_TTL, ACTIVE_TOOL_STALE_RUNNING_ROW_TTL, AutoSidebarPanel,
        AutoSidebarState, HotbarSlotState, SidebarAgentRow, SidebarFocus, SidebarHoverRow,
        SidebarHoverSection, SidebarHoverState, SidebarSubagentSummary, SidebarToolRow,
        SidebarWorkChecklistItem, SidebarWorkStrategyStep, SidebarWorkSummary, ToolRowOrder,
        agent_row_hover_text, auto_sidebar_panels, background_task_spinner_prefix,
        context_panel_cost_line, editorial_tool_rows, hotbar_panel_enabled,
        hotbar_panel_hover_texts, hotbar_panel_lines, hotbar_panel_slots, is_hotbar_disabled,
        normalize_activity_text, render_sidebar, sidebar_agent_rows, sidebar_hover_rows,
        sidebar_work_summary, sort_sidebar_agent_rows_as_tree, subagent_panel_hover_texts,
        subagent_panel_lines, subagent_panel_rows, task_panel_hover_texts, task_panel_lines,
        task_panel_row_sets, task_panel_rows, work_panel_empty_hint, work_panel_hover_texts,
        work_panel_lines,
    };
    use crate::config::Config;
    use crate::palette;
    use crate::palette::PaletteMode;
    use crate::tools::plan::StepStatus;
    use crate::tools::todo::TodoStatus;
    use crate::tui::active_cell::ActiveCell;
    use crate::tui::app::{
        AgentProgressMeta, App, AppMode, HuntVerdict, TaskPanelEntry, TaskPanelEntryKind,
        TuiOptions,
    };
    use crate::tui::history::{
        ExecCell, ExecSource, GenericToolCell, HistoryCell, ToolCell, ToolStatus,
    };
    use crate::tui::spinner::BRAILLE_SPINNER_FRAME_MS;
    use ratatui::{Terminal, backend::TestBackend, text::Line};
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
    fn context_panel_cost_line_shows_na_for_unpriced_zero_cost_model() {
        let mut app = create_test_app();
        app.model = "unknown-provider/unknown-model".to_string();

        assert_eq!(
            context_panel_cost_line(&app),
            "cost: n/a (no pricing data for unknown-provider/unknown-model)"
        );
    }

    #[test]
    fn context_panel_cost_line_uses_usd_for_usd_only_model_in_cny_mode() {
        let mut app = create_test_app();
        app.model = "kimi-k2.6".to_string();
        app.cost_currency = crate::pricing::CostCurrency::Cny;
        app.accrue_session_cost_estimate(crate::pricing::CostEstimate::usd_only(0.42));

        let line = context_panel_cost_line(&app);

        assert!(line.contains("$0.42"), "expected USD amount, got {line:?}");
        assert!(
            !line.contains('¥'),
            "must not render CNY zero, got {line:?}"
        );
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
    fn is_hotbar_disabled_only_for_an_explicit_empty_array() {
        // A missing `hotbar` key means "use defaults" — NOT disabled.
        assert!(!is_hotbar_disabled(&Config::default()));

        // An explicit `hotbar = []` is the disabled state.
        let disabled = Config {
            hotbar: Some(Vec::new()),
            ..Config::default()
        };
        assert!(is_hotbar_disabled(&disabled));

        // Real bindings are never disabled.
        let active = Config {
            hotbar: Some(vec![codewhale_config::HotbarBindingToml {
                slot: 1,
                action: "mode.plan".to_string(),
                label: None,
            }]),
            ..Config::default()
        };
        assert!(!is_hotbar_disabled(&active));
    }

    #[test]
    fn hotbar_panel_hidden_for_fresh_default_config() {
        // #3807: a fresh config has no `hotbar` key, so the panel is hidden
        // until the user opts in. Slot resolution + active state are covered by
        // `hotbar_panel_slots_resolve_configured_bindings_and_active_state`.
        let mut app = create_test_app();
        app.mode = AppMode::Agent;
        app.sidebar_focus = SidebarFocus::Pinned;

        assert!(
            !hotbar_panel_enabled(&app, &Config::default()),
            "fresh config must not enable the Hotbar panel"
        );
        assert!(
            hotbar_panel_slots(&app, &Config::default())
                .iter()
                .all(|slot| slot.state == HotbarSlotState::Empty),
            "fresh config resolves to no bound slots"
        );
    }

    #[test]
    fn hotbar_panel_slots_resolve_configured_bindings_and_active_state() {
        let mut app = create_test_app();
        app.mode = AppMode::Agent;
        app.sidebar_focus = SidebarFocus::Pinned;
        let config = Config {
            hotbar: Some(
                codewhale_config::default_hotbar_bindings()
                    .into_iter()
                    .map(|binding| codewhale_config::HotbarBindingToml {
                        slot: binding.slot,
                        action: binding.action,
                        label: binding.label,
                    })
                    .collect(),
            ),
            ..Config::default()
        };

        assert!(hotbar_panel_enabled(&app, &config));

        let slots = hotbar_panel_slots(&app, &config);

        assert_eq!(slots.len(), 8);
        assert_eq!(slots[0].slot, 1);
        assert_eq!(slots[0].label, "voice");
        assert_eq!(slots[0].state, HotbarSlotState::Inactive);
        assert_eq!(slots[3].label, "agent");
        assert_eq!(slots[3].state, HotbarSlotState::Active);
        assert!(slots[3].full_text.contains("mode.agent"));
        assert!(slots[3].full_text.contains("active"));
        let slot_4_chord = format!("{}4", crate::tui::widgets::key_hint::alt_prefix());
        assert!(slots[3].full_text.contains(&slot_4_chord));
        assert_eq!(
            slots[6].state,
            HotbarSlotState::Active,
            "sidebar toggle should be marked active when the sidebar is visible"
        );
        assert!(
            slots.iter().all(|slot| !matches!(
                slot.state,
                HotbarSlotState::Empty | HotbarSlotState::Unknown
            )),
            "default config should fill all eight slots: {slots:?}"
        );
    }

    #[test]
    fn hotbar_panel_slots_handle_empty_partial_and_unknown_config() {
        let app = create_test_app();
        let empty_config = Config {
            hotbar: Some(Vec::new()),
            ..Config::default()
        };

        let empty_slots = hotbar_panel_slots(&app, &empty_config);

        assert!(
            empty_slots
                .iter()
                .all(|slot| slot.state == HotbarSlotState::Empty),
            "explicit empty hotbar config should leave all slots empty: {empty_slots:?}"
        );

        let partial_config = Config {
            hotbar: Some(vec![
                codewhale_config::HotbarBindingToml {
                    slot: 2,
                    action: "mode.plan".to_string(),
                    label: Some("Plan!".to_string()),
                },
                codewhale_config::HotbarBindingToml {
                    slot: 5,
                    action: "plugin.missing".to_string(),
                    label: Some("Ghost".to_string()),
                },
            ]),
            ..Config::default()
        };

        let partial_slots = hotbar_panel_slots(&app, &partial_config);

        assert_eq!(partial_slots[0].state, HotbarSlotState::Empty);
        assert_eq!(partial_slots[1].state, HotbarSlotState::Inactive);
        assert_eq!(partial_slots[1].label, "Plan!");
        assert_eq!(partial_slots[4].state, HotbarSlotState::Unknown);
        assert_eq!(partial_slots[4].label, "Ghost");
        assert!(
            partial_slots[4]
                .full_text
                .contains("unknown action plugin.missing"),
            "unknown action should remain inspectable: {partial_slots:?}"
        );
    }

    #[test]
    fn hotbar_panel_lines_keep_two_fixed_rows_and_hover_status() {
        let mut app = create_test_app();
        app.mode = AppMode::Agent;
        app.sidebar_focus = SidebarFocus::Pinned;
        let config = Config {
            hotbar: Some(
                codewhale_config::default_hotbar_bindings()
                    .into_iter()
                    .map(|binding| codewhale_config::HotbarBindingToml {
                        slot: binding.slot,
                        action: binding.action,
                        label: binding.label,
                    })
                    .collect(),
            ),
            ..Config::default()
        };
        let slots = hotbar_panel_slots(&app, &config);

        let lines = hotbar_panel_lines(&slots, 32, &app.ui_theme);
        let text = lines_to_text(&lines);
        let hover = hotbar_panel_hover_texts(&slots);

        assert_eq!(text.len(), 2);
        assert!(
            text.iter()
                .all(|line| unicode_width::UnicodeWidthStr::width(line.as_str()) <= 32),
            "hotbar lines must stay within the sidebar content width: {text:?}"
        );
        assert!(
            text[0].contains("Alt1"),
            "first row should show slot 1: {text:?}"
        );
        assert!(
            text[0].contains("Alt4:*"),
            "active slot should be visibly marked in the fixed grid: {text:?}"
        );
        assert_eq!(hover.len(), 2);
        let slot_4_chord = format!("{}4", crate::tui::widgets::key_hint::alt_prefix());
        assert!(
            hover[0].contains(&slot_4_chord) && hover[0].contains("Slot 4: agent active"),
            "row hover text should expose active status: {hover:?}"
        );
    }

    #[test]
    fn sidebar_hotbar_render_smoke_omits_panel_when_empty_config() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Pinned;
        app.mode = AppMode::Agent;
        let config = Config {
            hotbar: Some(Vec::new()),
            ..Config::default()
        };

        let backend = TestBackend::new(44, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &mut app, &config))
            .expect("draw sidebar");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            !rendered.contains("Hotbar"),
            "empty hotbar config should not render hotbar panel: {rendered:?}"
        );
    }

    #[test]
    fn sidebar_hotbar_render_smoke_paints_default_slots() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Pinned;
        app.mode = AppMode::Agent;
        // #3807: the panel is hidden on a fresh config, so opt in explicitly
        // with the default bindings to smoke-test the rendered panel.
        let config = Config {
            hotbar: Some(codewhale_config::default_hotbar_bindings_toml()),
            ..Config::default()
        };

        let backend = TestBackend::new(44, 12);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &mut app, &config))
            .expect("draw sidebar");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("Hotbar"),
            "hotbar panel title missing: {rendered:?}"
        );
        let hotbar_range = format!("{}1-8", crate::tui::widgets::key_hint::alt_prefix());
        assert!(
            rendered.contains(&hotbar_range),
            "hotbar panel title should expose the accelerator: {rendered:?}"
        );
        assert!(
            rendered.contains("Alt1"),
            "slot 1 default binding should render: {rendered:?}"
        );
        assert!(
            rendered.contains("Alt4"),
            "active agent-mode slot should render distinctly: {rendered:?}"
        );
    }

    #[test]
    fn pinned_sidebar_renders_agents_section_when_subagents_are_active() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Pinned;
        app.subagent_cache
            .push(cached_agent("agent-active-1", Some("critic")));
        app.agent_progress.insert(
            "agent-active-1".to_string(),
            "checking sidebar visibility".to_string(),
        );

        let backend = TestBackend::new(72, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let config = Config::default();
        terminal
            .draw(|frame| render_sidebar(frame, frame.area(), &mut app, &config))
            .expect("draw sidebar");
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(
            rendered.contains("Agents"),
            "pinned sidebar must surface active sub-agents: {rendered:?}"
        );
        assert!(
            rendered.contains("critic") || rendered.contains("Agent 1"),
            "pinned sidebar should render the child agent label: {rendered:?}"
        );
        assert!(
            rendered.contains("checking sidebar visibility"),
            "pinned sidebar should render child progress: {rendered:?}"
        );
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
        assert!(
            text.iter().any(|line| line == "Strategy context"),
            "strategy should be grouped as context for the checklist: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("phase done: Simplify sidebar")),
            "completed strategy steps should render as phase context: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("phase next: Update prompts")),
            "pending strategy steps should render as phase context: {text:?}"
        );
        assert!(
            !text
                .iter()
                .any(|line| line.contains("[✓] Simplify sidebar"))
                && !text.iter().any(|line| line.contains("[ ] Update prompts")),
            "strategy rows must not look like a second checklist when Work checklist exists: {text:?}"
        );
    }

    #[test]
    fn work_panel_hover_renders_strategy_as_context_while_checklist_incomplete() {
        let summary = SidebarWorkSummary {
            checklist_completion_pct: 0,
            checklist_items: vec![SidebarWorkChecklistItem {
                id: 1,
                content: "Wire tool execution".to_string(),
                status: TodoStatus::InProgress,
            }],
            strategy_explanation: Some("Keep strategy and checklist linked".to_string()),
            strategy_steps: vec![
                SidebarWorkStrategyStep {
                    text: "Map phase boundaries".to_string(),
                    status: StepStatus::Completed,
                    elapsed: String::new(),
                },
                SidebarWorkStrategyStep {
                    text: "Implement counted work".to_string(),
                    status: StepStatus::InProgress,
                    elapsed: String::new(),
                },
            ],
            ..SidebarWorkSummary::default()
        };

        let hover = work_panel_hover_texts(&summary, 80, 16);

        assert!(
            hover.iter().any(|line| line == "Strategy context"),
            "hover should name strategy as context when checklist exists: {hover:?}"
        );
        assert!(
            hover
                .iter()
                .any(|line| line.contains("phase done: Map phase boundaries")),
            "hover strategy rows should be phase context: {hover:?}"
        );
        assert!(
            hover
                .iter()
                .any(|line| line.contains("phase now: Implement counted work")),
            "hover should expose the active strategy phase without checklist markers: {hover:?}"
        );
        assert!(
            !hover
                .iter()
                .any(|line| line.contains("[✓] Map phase boundaries")),
            "hover strategy rows must not look like a second checklist: {hover:?}"
        );
    }

    #[test]
    fn work_panel_suppresses_stale_active_strategy_when_checklist_complete() {
        let summary = SidebarWorkSummary {
            checklist_completion_pct: 100,
            checklist_items: vec![
                SidebarWorkChecklistItem {
                    id: 1,
                    content: "Ship the fix".to_string(),
                    status: TodoStatus::Completed,
                },
                SidebarWorkChecklistItem {
                    id: 2,
                    content: "Run focused tests".to_string(),
                    status: TodoStatus::Completed,
                },
            ],
            strategy_explanation: Some("Old plan metadata".to_string()),
            strategy_steps: vec![
                SidebarWorkStrategyStep {
                    text: "Completed context".to_string(),
                    status: StepStatus::Completed,
                    elapsed: String::new(),
                },
                SidebarWorkStrategyStep {
                    text: "Stale active phase".to_string(),
                    status: StepStatus::InProgress,
                    elapsed: String::new(),
                },
                SidebarWorkStrategyStep {
                    text: "Stale next phase".to_string(),
                    status: StepStatus::Pending,
                    elapsed: String::new(),
                },
            ],
            ..SidebarWorkSummary::default()
        };

        let display = lines_to_text(&work_panel_lines(
            &summary,
            80,
            16,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));
        let hover = work_panel_hover_texts(&summary, 80, 16);

        for rendered in [&display, &hover] {
            assert!(
                rendered
                    .iter()
                    .any(|line| line.contains("phase done: Completed context")),
                "completed strategy context may still render: {rendered:?}"
            );
            assert!(
                !rendered.iter().any(|line| line.contains("phase now:")),
                "stale in-progress strategy must not render as active work: {rendered:?}"
            );
            assert!(
                !rendered.iter().any(|line| line.contains("phase next:")),
                "stale pending strategy must not render as upcoming work: {rendered:?}"
            );
        }
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
    fn work_panel_overflow_hover_lists_omitted_checklist_items() {
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

        let hover = work_panel_hover_texts(&summary, 80, 6);
        let overflow = hover
            .iter()
            .find(|text| text.starts_with('+'))
            .expect("overflow hover row should exist");

        // Every checklist item is reachable: either as its own hover row or
        // listed inside the overflow row's hover text (#3063).
        for id in 1..=8 {
            let needle = format!("#{id} Release task {id}");
            assert!(
                hover.iter().any(|text| text.contains(&needle)),
                "item {id} should be inspectable via hover: {hover:?}"
            );
        }
        assert!(
            overflow.lines().count() > 1,
            "overflow hover should enumerate omitted items: {overflow:?}"
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
    fn sidebar_work_summary_uses_paused_quarry_when_goal_is_cleared() {
        let mut app = create_test_app();
        app.hunt.quarry = None;
        app.paused = true;
        app.paused_quarry = Some("Scan nested git repositories".to_string());

        let summary = sidebar_work_summary(&mut app);

        assert_eq!(
            summary.goal_objective.as_deref(),
            Some("Scan nested git repositories")
        );
        assert_eq!(summary.pause_indicator.as_deref(), Some("(Paused)"));
        assert!(summary.workflow_paused);
    }

    #[test]
    fn work_panel_renders_paused_command_goal() {
        let mut app = create_test_app();
        app.hunt.quarry = None;
        app.paused = false;
        app.paused_quarry = Some("Deploy to staging".to_string());

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
    fn tasks_panel_renders_active_tool_rows_before_background_empty_state() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "agent".to_string(),
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
            text.iter().any(|line| line.contains("[~] agent")),
            "active agent row missing: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("No active tasks")),
            "old empty state should not render during active tools: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_renders_recent_completed_tool_rows() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
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
        app.sidebar_focus = SidebarFocus::Tasks;
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
        app.sidebar_focus = SidebarFocus::Tasks;
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
    fn task_panel_row_sets_dedup_background_only_when_tasks_focused() {
        let mut app = create_test_app();
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "rlm_search".to_string(),
                status: ToolStatus::Running,
                input_summary: Some("scanning workspace".to_string()),
                output: None,
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);
        app.task_panel.push(TaskPanelEntry {
            id: "rlm-123".to_string(),
            status: "running".to_string(),
            prompt_summary: "RLM: scanning workspace".to_string(),
            duration_ms: Some(1_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        app.sidebar_focus = SidebarFocus::Tasks;
        let focused = task_panel_row_sets(&app);
        assert!(
            focused.background.is_empty(),
            "Tasks focus dedups background jobs against live tools: {:?}",
            focused.background
        );

        app.sidebar_focus = SidebarFocus::Auto;
        let auto = task_panel_row_sets(&app);
        assert_eq!(
            auto.background.len(),
            1,
            "Auto mode keeps background jobs even when a live tool matches"
        );
    }

    #[test]
    fn task_panel_rows_and_hover_share_one_snapshot() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.runtime_turn_id = Some("turn_abcdef123456".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_counter = 3;
        app.task_panel.push(TaskPanelEntry {
            id: "job_123".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test --workspace".to_string(),
            duration_ms: Some(12_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let row_sets = task_panel_row_sets(&app);
        let (lines, actions) = task_panel_rows(&app, &row_sets, 80, 12);
        let hover = task_panel_hover_texts(&app, &row_sets, 12);
        let text = lines_to_text(&lines);

        assert_eq!(lines.len(), actions.len(), "actions align with lines");
        let job_idx = text
            .iter()
            .position(|line| line.contains("Bash running"))
            .unwrap_or_else(|| panic!("bash job row missing: {text:?}"));
        assert!(
            hover[job_idx].contains("cargo test --workspace"),
            "hover text at the job row index carries the full command: {hover:?}"
        );
        assert!(
            text[0].starts_with("Turn 3"),
            "line shows stable turn label: {:?}",
            text[0]
        );
        assert!(
            hover[0].contains("turn_abcdef123456"),
            "hover carries the full turn id: {:?}",
            hover[0]
        );
    }

    #[test]
    fn tasks_panel_collapses_stale_running_tool_rows() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        let mut active = ActiveCell::new();
        for (idx, command) in ["long one", "long two"].into_iter().enumerate() {
            active.push_tool(
                format!("shell-{idx}"),
                HistoryCell::Tool(ToolCell::Exec(ExecCell {
                    command: command.to_string(),
                    status: ToolStatus::Running,
                    output: None,
                    live_output: None,
                    shell_task_id: None,
                    owner_agent_id: None,
                    owner_agent_name: None,
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
        app.sidebar_focus = SidebarFocus::Tasks;
        let mut active = ActiveCell::new();
        active.push_tool(
            "shell-1",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "cargo test --workspace".to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
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
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
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
            !text.iter().any(|line| line.contains("Bash jobs")),
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
            duration_ms: Some(0),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let text = lines_to_text(&task_panel_lines(&app, 96, 8));

        assert!(
            text.iter()
                .any(|line| line.contains("running cargo test --workspace --all-features")),
            "background shell headline should show the command, not only the shell id: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("⠋ Bash running")),
            "running background shell should show a braille spinner prefix: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("shell_33a08c3c")),
            "shell id should remain available as detail: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_attributes_subagent_owned_shell_jobs() {
        let mut app = create_test_app();
        app.task_panel.push(TaskPanelEntry {
            id: "shell_owned".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test -p codewhale-tui".to_string(),
            duration_ms: Some(2_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: Some("agent_verifier".to_string()),
            owner_agent_name: Some("verifier".to_string()),
        });

        let text = lines_to_text(&task_panel_lines(&app, 96, 8));

        assert!(
            text.iter().any(|line| line.contains("by verifier")),
            "owned shell job should show sub-agent attribution: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("shell_owned")),
            "shell id should remain visible with attribution: {text:?}"
        );
    }

    #[test]
    fn background_task_spinner_advances_at_readable_cadence() {
        let mut task = TaskPanelEntry {
            id: "shell_33a08c3c".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test".to_string(),
            duration_ms: Some(0),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        };

        assert_eq!(background_task_spinner_prefix(&task), Some("⠋"));

        task.duration_ms = Some(BRAILLE_SPINNER_FRAME_MS - 1);
        assert_eq!(background_task_spinner_prefix(&task), Some("⠋"));

        task.duration_ms = Some(BRAILLE_SPINNER_FRAME_MS);
        assert_eq!(background_task_spinner_prefix(&task), Some("⠙"));
    }

    #[test]
    fn tasks_panel_renders_model_reasoning_outside_background_commands() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.task_panel.push(TaskPanelEntry {
            id: "reasoning-1".to_string(),
            status: "running".to_string(),
            prompt_summary: "model reasoning".to_string(),
            duration_ms: Some(4_200),
            kind: TaskPanelEntryKind::ModelReasoning,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let text = lines_to_text(&task_panel_lines(&app, 80, 8));

        assert!(
            text.iter().any(|line| line == "Model reasoning"),
            "reasoning section missing: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("thinking running 4.2s")),
            "reasoning row should show live thinking duration: {text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("Bash jobs")),
            "reasoning must not be counted as a background command: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_auto_mode_shows_only_live_background_jobs() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Auto;
        app.runtime_turn_id = Some("turn_abcdef123456".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        let mut active = ActiveCell::new();
        active.push_tool(
            "tool-1",
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "read_file".to_string(),
                status: ToolStatus::Running,
                input_summary: Some("src/main.rs".to_string()),
                output: None,
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            })),
        );
        app.active_cell = Some(active);
        app.history
            .push(HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: "grep_files".to_string(),
                status: ToolStatus::Success,
                input_summary: Some("pattern: AgentProgress".to_string()),
                output: Some("found".to_string()),
                prompts: None,
                spillover_path: None,
                output_summary: Some("found AgentProgress".to_string()),
                is_diff: false,
            })));
        app.task_panel.push(TaskPanelEntry {
            id: "reasoning-1".to_string(),
            status: "running".to_string(),
            prompt_summary: "model reasoning".to_string(),
            duration_ms: Some(4_200),
            kind: TaskPanelEntryKind::ModelReasoning,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });
        app.task_panel.push(TaskPanelEntry {
            id: "shell_live".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test -p codewhale-tui".to_string(),
            duration_ms: Some(0),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let text = lines_to_text(&task_panel_lines(&app, 96, 12));

        assert!(
            text.iter().any(|line| line == "Bash jobs: 1 running"),
            "auto Tasks should keep live background jobs visible: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("⠋ Bash running")),
            "auto Tasks should animate running background jobs: {text:?}"
        );
        for hidden in [
            "Turn",
            "Live tools",
            "Model reasoning",
            "Recent tools",
            "[~] read_file",
            "[✓] grep_files",
            "thinking",
        ] {
            assert!(
                !text.iter().any(|line| line.contains(hidden)),
                "auto Tasks should not show {hidden:?}: {text:?}"
            );
        }
    }

    #[test]
    fn task_panel_actions_make_single_background_job_clickable() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.task_panel.push(TaskPanelEntry {
            id: "shell_only".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo build".to_string(),
            duration_ms: Some(1_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let (lines, actions) = task_panel_rows(&app, &task_panel_row_sets(&app), 80, 12);
        let text = lines_to_text(&lines);
        assert_eq!(lines.len(), actions.len());

        let label_idx = text
            .iter()
            .position(|line| line.contains("cargo build"))
            .expect("background job label row");
        assert!(
            text[label_idx].ends_with("[x]"),
            "running job label row exposes a compact stop target: {text:?}"
        );
        assert_eq!(
            actions[label_idx].as_deref(),
            Some("/jobs show shell_only"),
            "single-job label row must be clickable: {actions:?}"
        );
        assert_eq!(
            actions[label_idx + 1].as_deref(),
            Some("/jobs cancel shell_only"),
            "single-job detail row must cancel that job: {actions:?}"
        );
    }

    #[test]
    fn stale_background_job_row_shows_no_output_warning_and_cancel_hint() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.task_panel.push(TaskPanelEntry {
            id: "shell_stale".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: sleep 300".to_string(),
            duration_ms: Some(61_000),
            kind: TaskPanelEntryKind::Background,
            stale: true,
            elapsed_since_output_ms: Some(61_000),
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let (lines, actions) = task_panel_rows(&app, &task_panel_row_sets(&app), 80, 12);
        let text = lines_to_text(&lines);

        assert!(
            text.iter()
                .any(|line| line.contains("stale") && line.contains("no output")),
            "stale shell job should call out no-output state: {text:?}"
        );
        let hint_idx = text
            .iter()
            .position(|line| line.contains("cancel stale job"))
            .expect("stale cancel hint");
        assert_eq!(
            actions[hint_idx].as_deref(),
            Some("/jobs cancel shell_stale")
        );
        let detail_idx = text
            .iter()
            .position(|line| line.contains("shell_stale"))
            .expect("stale job detail row");
        assert_eq!(
            actions[detail_idx].as_deref(),
            Some("/jobs cancel shell_stale"),
            "stale job detail row should still cancel the specific job"
        );
    }

    #[test]
    fn task_panel_actions_route_each_job_to_its_own_id() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.task_panel.push(TaskPanelEntry {
            id: "shell_aaa".to_string(),
            status: "running".to_string(),
            prompt_summary: "shell: cargo test --workspace".to_string(),
            duration_ms: Some(2_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });
        app.task_panel.push(TaskPanelEntry {
            id: "task_bbb".to_string(),
            status: "running".to_string(),
            prompt_summary: "summarize the release notes".to_string(),
            duration_ms: Some(3_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let (lines, actions) = task_panel_rows(&app, &task_panel_row_sets(&app), 96, 16);
        let text = lines_to_text(&lines);
        assert_eq!(lines.len(), actions.len());

        let header_idx = text
            .iter()
            .position(|line| line.starts_with("Bash jobs"))
            .expect("background header row");
        assert!(actions[header_idx].is_none(), "header is not clickable");

        let shell_idx = text
            .iter()
            .position(|line| line.contains("cargo test --workspace"))
            .expect("shell job label row");
        assert_eq!(
            actions[shell_idx].as_deref(),
            Some("/jobs show shell_aaa"),
            "shell jobs route through /jobs: {actions:?}"
        );
        assert_eq!(
            actions[shell_idx + 1].as_deref(),
            Some("/jobs cancel shell_aaa"),
            "shell job detail row cancels the SAME job: {actions:?}"
        );

        let task_idx = text
            .iter()
            .position(|line| line.contains("task_bbb"))
            .expect("task job label row");
        assert!(
            text[shell_idx].ends_with("[x]") && text[task_idx].ends_with("[x]"),
            "running background jobs show inline stop affordances: {text:?}"
        );
        assert_eq!(
            actions[task_idx].as_deref(),
            Some("/task show task_bbb"),
            "task-manager jobs route through /task: {actions:?}"
        );
        assert_eq!(
            actions[task_idx + 1].as_deref(),
            Some("/task cancel task_bbb"),
            "task job detail row cancels the SAME job: {actions:?}"
        );

        let hint_idx = text
            .iter()
            .position(|line| line.contains("Ctrl+X"))
            .expect("cancel-all hint row");
        assert_eq!(actions[hint_idx].as_deref(), Some("/jobs cancel-all"));
    }

    #[test]
    fn task_panel_finished_job_detail_row_shows_instead_of_cancels() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.task_panel.push(TaskPanelEntry {
            id: "shell_done".to_string(),
            status: "completed".to_string(),
            prompt_summary: "shell: cargo fmt".to_string(),
            duration_ms: Some(500),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let (lines, actions) = task_panel_rows(&app, &task_panel_row_sets(&app), 80, 12);
        let text = lines_to_text(&lines);

        let label_idx = text
            .iter()
            .position(|line| line.contains("cargo fmt"))
            .expect("completed job label row");
        assert_eq!(actions[label_idx].as_deref(), Some("/jobs show shell_done"));
        assert_eq!(
            actions[label_idx + 1].as_deref(),
            Some("/jobs show shell_done"),
            "finished jobs must not expose a cancel click target: {actions:?}"
        );
    }

    #[test]
    fn task_panel_actions_align_with_lines_when_live_tools_present() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.runtime_turn_id = Some("0196f0a3-aaaa-bbbb-cccc-ddddeeee0000".to_string());
        let mut active = ActiveCell::new();
        active.push_tool(
            "shell-1",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "sleep 600".to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: Some(Instant::now()),
                duration_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            })),
        );
        app.active_cell = Some(active);
        app.task_panel.push(TaskPanelEntry {
            id: "task_q".to_string(),
            status: "running".to_string(),
            prompt_summary: "investigate flaky test".to_string(),
            duration_ms: Some(9_000),
            kind: TaskPanelEntryKind::Background,
            stale: false,
            elapsed_since_output_ms: None,
            owner_agent_id: None,
            owner_agent_name: None,
        });

        let (lines, actions) = task_panel_rows(&app, &task_panel_row_sets(&app), 96, 16);
        let text = lines_to_text(&lines);
        assert_eq!(
            lines.len(),
            actions.len(),
            "actions must stay index-aligned with lines: {text:?}"
        );

        // Turn label and live-tool rows are not clickable.
        assert!(actions[0].is_none(), "turn label row has no action");
        let live_idx = text
            .iter()
            .position(|line| line == "Live tools")
            .expect("live tools header");
        assert!(actions[live_idx].is_none());

        let task_idx = text
            .iter()
            .position(|line| line.contains("task_q"))
            .expect("background job label row");
        assert_eq!(actions[task_idx].as_deref(), Some("/task show task_q"));
    }

    #[test]
    fn subagent_panel_actions_mark_agent_rows_with_role_mix_header() {
        let mut role_counts = std::collections::BTreeMap::new();
        role_counts.insert("worker".to_string(), 1);
        let summary = SidebarSubagentSummary {
            cached_total: 1,
            cached_running: 1,
            role_counts,
            ..SidebarSubagentSummary::default()
        };
        let rows = vec![SidebarAgentRow {
            id: "agent_0123456789".to_string(),
            parent_run_id: None,
            spawn_depth: 1,
            name: "investigator".to_string(),
            role: "worker".to_string(),
            status: "running".to_string(),
            objective: None,
            git_branch: None,
            progress: Some("scanning".to_string()),
            steps_taken: 2,
            duration_ms: Some(1_000),
        }];

        let (lines, actions) = subagent_panel_rows(&summary, &rows, 48, 8, &palette::UI_THEME);
        let text = lines_to_text(&lines);
        assert_eq!(lines.len(), actions.len());

        assert!(actions[0].is_none(), "count header has no action");
        assert!(actions[1].is_none(), "role-mix header has no action");
        let agent_idx = text
            .iter()
            .position(|line| line.contains("investigator"))
            .expect("agent label row");
        assert_eq!(actions[agent_idx].as_deref(), Some("/fleet status"));
        assert!(
            actions[agent_idx + 1].is_none(),
            "agent detail row has no action"
        );
    }

    #[test]
    fn subagent_panel_actions_skip_role_mix_slot_for_progress_only_agents() {
        // Progress-only agents have no cached role counts, so there is no
        // role-mix line — the first agent row sits directly under the count
        // header and must still resolve to /fleet status (#3028 audit fix).
        let summary = SidebarSubagentSummary {
            progress_only_count: 1,
            ..SidebarSubagentSummary::default()
        };
        let rows = vec![SidebarAgentRow {
            id: "agent_fedcba987654".to_string(),
            parent_run_id: None,
            spawn_depth: 1,
            name: "scout".to_string(),
            role: "explorer".to_string(),
            status: "running".to_string(),
            objective: None,
            git_branch: None,
            progress: Some("reading".to_string()),
            steps_taken: 1,
            duration_ms: None,
        }];

        let (lines, actions) = subagent_panel_rows(&summary, &rows, 48, 8, &palette::UI_THEME);
        let text = lines_to_text(&lines);
        assert_eq!(lines.len(), actions.len());

        assert!(actions[0].is_none(), "count header has no action");
        let agent_idx = text
            .iter()
            .position(|line| line.contains("scout"))
            .expect("agent label row");
        assert_eq!(
            agent_idx, 1,
            "no role-mix line should be emitted without role counts: {text:?}"
        );
        assert_eq!(actions[agent_idx].as_deref(), Some("/fleet status"));
    }

    #[test]
    fn sort_sidebar_agent_rows_as_tree_emits_each_row_exactly_once() {
        let agent_row = |id: &str, parent: Option<&str>| SidebarAgentRow {
            id: id.to_string(),
            parent_run_id: parent.map(str::to_string),
            spawn_depth: 1,
            name: id.to_string(),
            role: "explore".to_string(),
            status: "running".to_string(),
            objective: None,
            git_branch: None,
            progress: None,
            steps_taken: 1,
            duration_ms: None,
        };
        // Parent + child + a two-node parent cycle: the cycle has no root, so
        // only the orphan sweep reaches it. Every row must still come out
        // exactly once (the move-based materialization panics on a double
        // take and this pins the drop case too).
        let rows = vec![
            agent_row("agent_parent", None),
            agent_row("agent_child", Some("agent_parent")),
            agent_row("agent_cycle_a", Some("agent_cycle_b")),
            agent_row("agent_cycle_b", Some("agent_cycle_a")),
        ];

        let sorted = sort_sidebar_agent_rows_as_tree(rows);

        assert_eq!(sorted.len(), 4, "no rows dropped or duplicated");
        let mut ids: Vec<&str> = sorted.iter().map(|row| row.id.as_str()).collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            vec![
                "agent_child",
                "agent_cycle_a",
                "agent_cycle_b",
                "agent_parent"
            ]
        );
        assert_eq!(sorted[0].id, "agent_parent");
        assert_eq!(sorted[1].id, "agent_child");
    }

    #[test]
    fn subagent_panel_collapses_terminal_non_done_rows() {
        let summary = SidebarSubagentSummary {
            cached_total: 3,
            cached_running: 0,
            ..SidebarSubagentSummary::default()
        };
        let rows = ["canceled", "failed", "interrupted"]
            .into_iter()
            .enumerate()
            .map(|(idx, status)| SidebarAgentRow {
                id: format!("agent_terminal_{idx}"),
                parent_run_id: None,
                spawn_depth: 1,
                name: format!("worker-{idx}"),
                role: "explore".to_string(),
                status: status.to_string(),
                objective: None,
                git_branch: None,
                progress: Some(format!("{status} with a long stale-looking detail")),
                steps_taken: 7,
                duration_ms: Some(1_000),
            })
            .collect::<Vec<_>>();

        let (lines, _) = subagent_panel_rows(&summary, &rows, 72, 10, &palette::UI_THEME);
        let text = lines_to_text(&lines);

        assert!(
            text.iter().any(|line| line.contains("3 done")),
            "terminal summary remains visible: {text:?}"
        );
        for idx in 0..3 {
            assert!(
                text.iter()
                    .any(|line| line.contains(&format!("worker-{idx}"))),
                "terminal worker label remains visible: {text:?}"
            );
        }
        assert!(
            !text.iter().any(|line| line.contains("step(s)")),
            "terminal rows should not keep noisy detail lines: {text:?}"
        );
        assert!(
            !text
                .iter()
                .any(|line| line.contains("stale-looking detail")),
            "terminal rows should hide stale progress details: {text:?}"
        );
    }

    #[test]
    fn subagent_sidebar_orders_and_indents_nested_children() {
        let rows = vec![
            SidebarAgentRow {
                id: "agent_grandchild".to_string(),
                parent_run_id: Some("agent_parent".to_string()),
                spawn_depth: 2,
                name: "nested-reader".to_string(),
                role: "explore".to_string(),
                status: "done".to_string(),
                objective: None,
                git_branch: None,
                progress: None,
                steps_taken: 1,
                duration_ms: Some(250),
            },
            SidebarAgentRow {
                id: "agent_parent".to_string(),
                parent_run_id: None,
                spawn_depth: 1,
                name: "nested-parent".to_string(),
                role: "explore".to_string(),
                status: "running".to_string(),
                objective: None,
                git_branch: None,
                progress: Some("waiting on child".to_string()),
                steps_taken: 2,
                duration_ms: Some(500),
            },
        ];
        let sorted = sort_sidebar_agent_rows_as_tree(rows);
        assert_eq!(sorted[0].id, "agent_parent");
        assert_eq!(sorted[1].id, "agent_grandchild");
        assert_eq!(sorted.len(), 2, "tree sort must not drop or duplicate rows");

        let summary = SidebarSubagentSummary {
            cached_total: 2,
            cached_running: 1,
            ..SidebarSubagentSummary::default()
        };
        let (lines, _) = subagent_panel_rows(&summary, &sorted, 64, 8, &palette::UI_THEME);
        let text = lines_to_text(&lines);
        let parent_idx = text
            .iter()
            .position(|line| line.contains("nested-parent"))
            .expect("parent row");
        let child_idx = text
            .iter()
            .position(|line| line.contains("nested-reader"))
            .expect("child row");
        assert!(
            parent_idx < child_idx,
            "parent must render before child: {text:?}"
        );
        assert!(
            text[child_idx].contains("└─"),
            "nested child should render with a tree branch marker: {text:?}"
        );

        let hover = agent_row_hover_text(&sorted[1]);
        assert!(hover.contains("parent: agent_parent"));
        assert!(hover.contains("depth: 2"));
    }

    #[test]
    fn subagent_sidebar_orders_and_indents_live_progress_children() {
        let mut app = create_test_app();
        app.agent_progress
            .insert("agent_parent".to_string(), "running".to_string());
        app.agent_progress_meta.insert(
            "agent_parent".to_string(),
            AgentProgressMeta {
                parent_run_id: None,
                spawn_depth: 1,
            },
        );
        app.agent_progress.insert(
            "agent_child".to_string(),
            "step 2: finished tool 'read_file'".to_string(),
        );
        app.agent_progress_meta.insert(
            "agent_child".to_string(),
            AgentProgressMeta {
                parent_run_id: Some("agent_parent".to_string()),
                spawn_depth: 2,
            },
        );
        app.ensure_agent_label("agent_parent");
        app.ensure_agent_label("agent_child");

        let rows = sidebar_agent_rows(&app);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "agent_parent");
        assert_eq!(rows[1].id, "agent_child");
        assert_eq!(rows[1].parent_run_id.as_deref(), Some("agent_parent"));
        assert_eq!(rows[1].spawn_depth, 2);
        assert_eq!(rows[1].role, "child");

        let summary = SidebarSubagentSummary {
            progress_only_count: 2,
            ..SidebarSubagentSummary::default()
        };
        let (lines, _) = subagent_panel_rows(&summary, &rows, 64, 8, &palette::UI_THEME);
        let text = lines_to_text(&lines);
        let parent_idx = text
            .iter()
            .position(|line| line.contains("Agent 1"))
            .expect("live parent row");
        let child_idx = text
            .iter()
            .position(|line| line.contains("Agent 2"))
            .expect("live child row");
        assert!(
            parent_idx < child_idx,
            "live parent must render before child: {text:?}"
        );
        assert!(
            text[child_idx].contains("└─"),
            "live child should render with a tree branch marker: {text:?}"
        );

        let hover = agent_row_hover_text(&rows[1]);
        assert!(hover.contains("parent: agent_parent"));
        assert!(hover.contains("depth: 2"));
    }

    #[test]
    fn tasks_panel_collapses_repeated_low_value_recent_tools_after_failures() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
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
        app.sidebar_focus = SidebarFocus::Tasks;
        for _ in 0..3 {
            app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "cd /tmp/repo && sleep 15 && gh pr checks 1616 --repo Hmbown/CodeWhale"
                    .to_string(),
                status: ToolStatus::Failed,
                output: Some("Lint pending\nTest pending".to_string()),
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
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
        app.sidebar_focus = SidebarFocus::Tasks;
        app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo test -p codewhale-tui".to_string(),
            status: ToolStatus::Failed,
            output: Some("test failed".to_string()),
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
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
            text.iter().any(|line| line.contains(
                &crate::tui::key_shortcuts::tool_details_shortcut_action_hint("details")
            )),
            "failed row should include the next action: {text:?}"
        );
    }

    #[test]
    fn tasks_panel_keeps_duration_and_status_on_recent_shell_rows() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.history.push(HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: "cargo check".to_string(),
            status: ToolStatus::Success,
            output: Some("Finished".to_string()),
            live_output: None,
            shell_task_id: None,
            owner_agent_id: None,
            owner_agent_name: None,
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
        app.sidebar_focus = SidebarFocus::Tasks;
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
            text.iter().any(|line| line.contains("[~] wait Bash")),
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
        app.sidebar_focus = SidebarFocus::Tasks;
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
                .filter(|line| line.contains("[~] wait Bash"))
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
    fn tasks_panel_collapses_repeated_shell_waits_without_task_marker() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        let mut active = ActiveCell::new();
        for (id, summary) in [
            ("shell-wait-1", "Background task running (no new output)."),
            ("shell-wait-2", "Still running after 10s."),
        ] {
            active.push_tool(
                id,
                HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                    name: "task_shell_wait".to_string(),
                    status: ToolStatus::Running,
                    input_summary: None,
                    output: None,
                    prompts: None,
                    spillover_path: None,
                    output_summary: Some(summary.to_string()),
                    is_diff: false,
                })),
            );
        }
        app.active_cell = Some(active);

        let text = lines_to_text(&task_panel_lines(&app, 100, 8));

        assert_eq!(
            text.iter()
                .filter(|line| line.contains("[~] wait Bash"))
                .count(),
            1,
            "same wait helper without task markers should still collapse: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("2 waits collapsed")),
            "collapsed no-marker row should show the wait count: {text:?}"
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
                parent_run_id: None,
                spawn_depth: 1,
                name: "check-docs-mcp".to_string(),
                role: "explore".to_string(),
                status: "running".to_string(),
                objective: None,
                git_branch: Some("feature/docs".to_string()),
                progress: Some("step 2/3: running tool 'read_file'".to_string()),
                steps_taken: 2,
                duration_ms: Some(22_000),
            },
            SidebarAgentRow {
                id: "agent_850aa63f".to_string(),
                parent_run_id: None,
                spawn_depth: 1,
                name: "check-install-docs".to_string(),
                role: "general".to_string(),
                status: "done".to_string(),
                objective: None,
                git_branch: None,
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
        let wide_text = lines_to_text(&subagent_panel_lines(
            &summary,
            &rows,
            96,
            12,
            &palette::UI_THEME,
        ));
        assert!(
            wide_text.iter().any(|l| l.contains("branch feature/docs")),
            "branch detail missing at wide width: {wide_text:?}",
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

    #[test]
    fn work_hover_text_preserves_full_checklist_item() {
        let long_item =
            "Add ProviderKind::HuggingFace direct route with all auth and docs coverage";
        let summary = SidebarWorkSummary {
            checklist_completion_pct: 0,
            checklist_items: vec![SidebarWorkChecklistItem {
                id: 7,
                content: long_item.to_string(),
                status: TodoStatus::InProgress,
            }],
            ..SidebarWorkSummary::default()
        };

        let display = lines_to_text(&work_panel_lines(
            &summary,
            18,
            4,
            PaletteMode::Dark,
            &palette::UI_THEME,
        ));
        let hover = work_panel_hover_texts(&summary, 18, 4);

        assert!(
            display.iter().any(|line| line.contains("...")),
            "compact Work row should be ellipsized in this fixture: {display:?}"
        );
        assert!(
            hover.iter().any(|line| line.contains(long_item)),
            "hover text should retain the full checklist item: {hover:?}"
        );
    }

    #[test]
    fn sidebar_hover_rows_mark_source_text_diff_as_truncated() {
        use ratatui::layout::Rect;
        let display = vec!["[~] agent imple…".to_string()];
        let full = vec!["[~] agent implementation-worker-for-sidebar-detail-popover".to_string()];
        let rows = sidebar_hover_rows(Rect::new(62, 5, 16, 4), &display, &full, &[]);

        let expected = SidebarHoverRow {
            row_y: 5,
            display_text: display[0].clone(),
            full_text: full[0].clone(),
            detail: None,
            is_truncated: true,
            click_action: None,
            stop_action: None,
            stop_zone_start_col: None,
            stop_zone_end_col: None,
        };
        assert_eq!(rows, vec![expected]);
    }

    #[test]
    fn sidebar_hover_rows_assign_stop_zone_to_running_task_labels() {
        use ratatui::layout::Rect;

        let display = vec!["cargo test [x]".to_string(), "  running 1.00s".to_string()];
        let full = display.clone();
        let actions = vec![
            Some("/jobs show shell_x".to_string()),
            Some("/jobs cancel shell_x".to_string()),
        ];

        let rows = sidebar_hover_rows(Rect::new(60, 5, 20, 4), &display, &full, &actions);

        assert_eq!(rows[0].click_action.as_deref(), Some("/jobs show shell_x"));
        assert_eq!(rows[0].stop_action.as_deref(), Some("/jobs cancel shell_x"));
        assert_eq!(rows[0].stop_zone_start_col, Some(71));
        assert_eq!(rows[0].stop_zone_end_col, Some(74));
        assert!(rows[1].stop_action.is_none());
    }

    #[test]
    fn subagent_hover_text_preserves_full_agent_id_and_progress() {
        let mut role_counts = std::collections::BTreeMap::new();
        role_counts.insert("worker".to_string(), 1);
        let summary = SidebarSubagentSummary {
            cached_total: 1,
            cached_running: 1,
            role_counts,
            ..SidebarSubagentSummary::default()
        };
        let long_id = "019e9142-83f6-7713-87f1-28902e74bf05";
        let long_progress =
            "currently reviewing sidebar hover popover wrapping and hitbox metadata";
        let rows = vec![SidebarAgentRow {
            id: long_id.to_string(),
            parent_run_id: None,
            spawn_depth: 1,
            name: "sidebar-detail-worker-with-long-name".to_string(),
            role: "worker".to_string(),
            status: "running".to_string(),
            objective: None,
            git_branch: Some("codex/sidebar-hover".to_string()),
            progress: Some(long_progress.to_string()),
            steps_taken: 9,
            duration_ms: Some(12_345),
        }];

        let hover = subagent_panel_hover_texts(&summary, &rows, 5);
        assert!(
            hover.iter().any(|line| line.contains(long_id)),
            "hover text should include the full agent id: {hover:?}"
        );
        assert!(
            hover.iter().any(|line| line.contains(long_progress)),
            "hover text should include the full progress before popover wrapping: {hover:?}"
        );
    }

    #[test]
    fn subagent_label_hover_carries_full_agent_dossier() {
        let mut role_counts = std::collections::BTreeMap::new();
        role_counts.insert("worker".to_string(), 1);
        let summary = SidebarSubagentSummary {
            cached_total: 1,
            cached_running: 1,
            role_counts,
            ..SidebarSubagentSummary::default()
        };
        let rows = vec![SidebarAgentRow {
            id: "019e9142-83f6-7713-87f1-28902e74bf05".to_string(),
            parent_run_id: None,
            spawn_depth: 1,
            name: "doc-checker".to_string(),
            role: "worker".to_string(),
            status: "running".to_string(),
            objective: Some("Verify install docs against the release notes".to_string()),
            git_branch: Some("codex/doc-check".to_string()),
            progress: Some("step 2/3: running tool 'read_file'".to_string()),
            steps_taken: 2,
            duration_ms: Some(22_000),
        }];

        let hover = subagent_panel_hover_texts(&summary, &rows, 6);
        let label = hover
            .iter()
            .find(|text| text.contains("doc-checker"))
            .expect("label hover row should exist");

        assert!(
            label.contains("id: 019e9142-83f6-7713-87f1-28902e74bf05"),
            "label hover should carry the full id: {label:?}"
        );
        assert!(
            label.contains("status: running") && label.contains("elapsed"),
            "label hover should carry status and elapsed time: {label:?}"
        );
        assert!(
            label.contains("objective: Verify install docs against the release notes"),
            "label hover should carry the objective: {label:?}"
        );
        assert!(
            label.contains("branch: codex/doc-check"),
            "label hover should carry the branch: {label:?}"
        );
        assert!(
            label.contains("progress: step 2/3: running tool 'read_file'"),
            "label hover should carry untruncated progress: {label:?}"
        );
    }

    // ── #3030: stable labels instead of raw internal ids ───────────────────

    #[test]
    fn tasks_panel_shows_stable_turn_label_not_uuid() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.runtime_turn_id = Some("0196f0a3-1111-2222-3333-444455556666".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_counter = 3;

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));
        assert!(
            text[0].contains("Turn 3 (in_progress)"),
            "compact row must show the stable turn label: {text:?}"
        );
        assert!(
            !text[0].contains("0196f0a3"),
            "raw turn UUID must stay out of the compact row: {text:?}"
        );

        let hover = task_panel_hover_texts(&app, &task_panel_row_sets(&app), 8);
        assert!(
            hover[0].contains("0196f0a3-1111-2222-3333-444455556666"),
            "full turn UUID must remain available in hover text: {hover:?}"
        );
    }

    #[test]
    fn tasks_panel_turn_label_falls_back_before_first_counted_turn() {
        let mut app = create_test_app();
        app.sidebar_focus = SidebarFocus::Tasks;
        app.runtime_turn_id = Some("0196f0a3-1111-2222-3333-444455556666".to_string());
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_counter = 0;

        let text = lines_to_text(&task_panel_lines(&app, 64, 8));
        assert!(
            text[0].contains("Current turn (in_progress)"),
            "zero counter falls back to a generic label: {text:?}"
        );
    }

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

    fn cached_agent(
        agent_id: &str,
        nickname: Option<&str>,
    ) -> crate::tools::subagent::SubAgentResult {
        crate::tools::subagent::SubAgentResult {
            name: "implementation-worker".to_string(),
            agent_id: agent_id.to_string(),
            context_mode: "fresh".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: crate::tools::subagent::SubAgentType::General,
            assignment: crate::tools::subagent::SubAgentAssignment {
                objective: "task".to_string(),
                role: Some("worker".to_string()),
            },
            model: String::new(),
            nickname: nickname.map(str::to_string),
            status: crate::tools::subagent::SubAgentStatus::Running,
            worker_status: None,
            parent_run_id: None,
            spawn_depth: 0,
            result: None,
            steps_taken: 1,
            checkpoint: None,
            needs_input: None,
            duration_ms: 100,
            from_prior_session: false,
        }
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
    }

    #[test]
    fn sidebar_progress_only_rows_parse_status_instead_of_hardcoding_running() {
        let mut app = create_test_app();
        app.agent_progress.insert(
            "agent_queued".to_string(),
            "queued for launch permit".to_string(),
        );

        let rows = sidebar_agent_rows(&app);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "queued");
    }

    #[test]
    fn sidebar_agent_rows_prefer_nickname_over_generated_label() {
        let mut app = create_test_app();
        let agent_id = "agent_cafe0123";
        app.ensure_agent_label(agent_id);
        app.subagent_cache
            .push(cached_agent(agent_id, Some("doc-fixer")));

        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(
            rows[0].name, "doc-fixer",
            "user nickname must beat the generated Agent-N label"
        );

        // Without a nickname the generated label is used.
        app.subagent_cache[0].nickname = None;
        let rows = super::sidebar_agent_rows(&app);
        assert_eq!(rows[0].name, "Agent 1");
    }
}
