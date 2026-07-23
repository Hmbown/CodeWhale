//! Coherent shell grammar for the underwater TUI.
//!
//! This module owns phase, responsive density, the empty-state composition,
//! and the compact header/footer fact budget. Product data still belongs to
//! [`App`]; this is only its terminal projection. Keeping these decisions in
//! one place prevents the default UI from drifting back into a header +
//! sidebar + dashboard + footer composition with four owners for one fact.

use std::borrow::Cow;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::config::HeaderItem;
use crate::localization::{Locale, MessageId, tr};
use crate::tui::{
    app::{App, AppMode, OnboardingState},
    approval::ApprovalMode,
    footer_ui::format_token_count_compact,
    views::ModalKind,
};

/// Responsive density tier. It changes how much truth is shown, never the
/// underlying state grammar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTier {
    Compact,
    Normal,
    Wide,
}

const LAUNCH_ROWS: [(MessageId, &str); 5] = [
    (MessageId::LaunchMenuNewSession, "Enter"),
    (MessageId::LaunchMenuNewWorktree, "Ctrl+N"),
    (MessageId::LaunchMenuResumeSession, "Ctrl+R"),
    (MessageId::LaunchMenuChangelog, "Ctrl+L"),
    (MessageId::LaunchMenuQuit, "Ctrl+Q"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    None,
    NewSession,
    CreateWorktree(String),
    Resume,
    Changelog,
    Quit,
}

/// Translate launch-menu input into one product action. Direct reliable keys
/// and row navigation share this path, so the printed key column cannot drift
/// away from the handler.
pub fn handle_launch_key(
    launch: &mut crate::tui::app::LaunchState,
    key: KeyEvent,
    locale: Locale,
) -> LaunchAction {
    if let Some(input) = launch.worktree_input.as_mut() {
        return match key.code {
            KeyCode::Esc => {
                launch.worktree_input = None;
                launch.status = None;
                LaunchAction::None
            }
            KeyCode::Enter => {
                let name = input.trim().to_string();
                launch.worktree_input = None;
                LaunchAction::CreateWorktree(name)
            }
            KeyCode::Backspace => {
                input.pop();
                LaunchAction::None
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                launch.worktree_input = None;
                launch.status = None;
                LaunchAction::None
            }
            KeyCode::Char(ch)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                input.push(ch);
                LaunchAction::None
            }
            _ => LaunchAction::None,
        };
    }

    let direct = match key.code {
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(1),
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(2),
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(3),
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(4),
        _ => None,
    };
    if let Some(selected) = direct {
        launch.selected = selected;
    } else {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                launch.selected = launch.selected.saturating_sub(1);
                return LaunchAction::None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                launch.selected = (launch.selected + 1).min(LAUNCH_ROWS.len() - 1);
                return LaunchAction::None;
            }
            KeyCode::Enter => {}
            _ => return LaunchAction::None,
        }
    }

    match launch.selected {
        0 => LaunchAction::NewSession,
        1 if launch.worktree_available => {
            launch.worktree_input = Some(String::new());
            launch.status = Some(tr(locale, MessageId::LaunchWorktreePrompt).into_owned());
            LaunchAction::None
        }
        1 => {
            launch.status = Some(tr(locale, MessageId::LaunchWorktreeNeedsGit).into_owned());
            LaunchAction::None
        }
        2 => LaunchAction::Resume,
        3 => LaunchAction::Changelog,
        4 => LaunchAction::Quit,
        _ => LaunchAction::None,
    }
}

impl ShellTier {
    #[must_use]
    pub fn for_area(area: Rect) -> Self {
        if area.width < 60 || area.height < 16 {
            Self::Compact
        } else if area.width < 110 || area.height < 30 {
            Self::Normal
        } else {
            Self::Wide
        }
    }

    #[must_use]
    pub fn for_chrome_width(width: u16) -> Self {
        if width < 60 {
            Self::Compact
        } else if width < 110 {
            Self::Normal
        } else {
            Self::Wide
        }
    }
}

/// Perceptual session phase. Every treatment reads from this same enum so a
/// footer cannot say `idle` while the transcript is asking for approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPhase {
    Idle,
    Typing,
    Working,
    /// A live verification pass (tests/checks/lints). Same clock family as
    /// `Working` but rendered as the metered braille tick — checking, not
    /// searching (ocean state model).
    Verifying,
    Waiting,
    Approval,
    Done,
    Failed,
}

/// The one truthful verb shown while a turn is live. This deliberately stays
/// smaller than the tool taxonomy: the phase strip only needs to distinguish
/// hidden reasoning, read-shaped exploration, other tool use, verification,
/// and generic model work. It never exposes reasoning text or tool arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LiveActivityKind {
    Working,
    Reasoning,
    Reading,
    UsingTool,
    Verifying,
}

/// Bounded projection of live turn activity. Completed entries are ignored,
/// so an `ActiveCell` retained until `TurnComplete` cannot keep the shell in a
/// false working state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LiveActivity {
    kind: LiveActivityKind,
    running_tools: usize,
}

impl LiveActivity {
    #[must_use]
    pub(crate) fn from_app(app: &App) -> Self {
        let tools = running_tool_facts(app);
        let kind = if tools.verifying {
            LiveActivityKind::Verifying
        } else if tools.count > 0 && tools.all_reading {
            LiveActivityKind::Reading
        } else if tools.count > 0 {
            LiveActivityKind::UsingTool
        } else if app.streaming_thinking_active_entry.is_some() {
            LiveActivityKind::Reasoning
        } else {
            LiveActivityKind::Working
        };
        Self {
            kind,
            running_tools: tools.count,
        }
    }

    #[must_use]
    pub(crate) fn kind(self) -> LiveActivityKind {
        self.kind
    }

    #[must_use]
    pub(crate) fn running_tool_count(self) -> usize {
        self.running_tools
    }

    #[must_use]
    fn is_explicit(self) -> bool {
        !matches!(self.kind, LiveActivityKind::Working)
    }

    #[must_use]
    fn label(self, locale: Locale) -> Cow<'static, str> {
        match self.kind {
            LiveActivityKind::Working => tr(locale, MessageId::PhaseWorking),
            LiveActivityKind::Reasoning => tr(locale, MessageId::PhaseReasoning),
            LiveActivityKind::Reading => tr(locale, MessageId::PhaseReading),
            LiveActivityKind::UsingTool => tr(locale, MessageId::PhaseUsingTool),
            LiveActivityKind::Verifying => tr(locale, MessageId::PhaseVerifying),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RunningToolFacts {
    count: usize,
    all_reading: bool,
    verifying: bool,
}

impl Default for RunningToolFacts {
    fn default() -> Self {
        Self {
            count: 0,
            all_reading: true,
            verifying: false,
        }
    }
}

impl RunningToolFacts {
    fn observe(&mut self, reading: bool, verifying: bool) {
        self.count = self.count.saturating_add(1);
        self.all_reading &= reading;
        self.verifying |= verifying;
    }
}

const WORKING_BUBBLE_FRAMES: [&str; 8] = ["⠀", "⢀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣿"];
const COMPLETION_BREATH_MS: u128 = 800;
const COMPLETION_RELEASE_MS: u128 = 560;
const IDLE_WHALE_SPOUT_ROW: &str = "   ˚";
const IDLE_WHALE_ROWS: [&str; 3] = [
    "  ▗▄▄▄▄▄▄▄▄▄▄▄▖      ▚▞",
    " ▐██·███████████▙━━━━▞",
    "  ▝▀▀▀▀▀▀▀▀▀▀▀▀▘",
];

const UWU_IDLE_WHALE_SPOUT_ROW: &str = "   ˚✦";
const UWU_IDLE_WHALE_ROWS: [&str; 3] = [
    " ▗▄▄▄▄▄▄▄▄▄▄▄▖    ▚▞",
    "▐█░·░█████████▙▄▄▞",
    " ▝▀▀▀▀▀▀▀▀▀▀▀▘",
];

const IDLE_SHIMMER_CYCLE_MS: u128 = 4_000;
const IDLE_SHIMMER_SWEEP_FRACTION: f32 = 0.32;
const IDLE_SHIMMER_BAND_HALF_WIDTH: f32 = 0.38;
const IDLE_SHIMMER_STRENGTH: f32 = 0.33;

impl ShellPhase {
    #[must_use]
    pub fn from_app(app: &App) -> Self {
        Self::from_app_with_activity(app, LiveActivity::from_app(app))
    }

    #[must_use]
    pub(crate) fn from_app_with_activity(app: &App, activity: LiveActivity) -> Self {
        if matches!(
            app.view_stack.top_kind(),
            Some(
                ModalKind::Approval
                    | ModalKind::Elevation
                    | ModalKind::UserInput
                    | ModalKind::PlanPrompt
            )
        ) {
            return Self::Approval;
        }
        if app.turn_error_posted
            || matches!(app.runtime_turn_status.as_deref(), Some("failed" | "error"))
        {
            return Self::Failed;
        }
        if app.pending_user_input_prompt.is_some()
            || app.plan_prompt_pending
            || app
                .task_panel
                .iter()
                .any(|task| matches!(task.status.as_str(), "waiting" | "needs_user"))
        {
            return Self::Waiting;
        }
        if app.is_loading
            || matches!(app.runtime_turn_status.as_deref(), Some("in_progress"))
            || activity.is_explicit()
        {
            if activity.kind() == LiveActivityKind::Verifying {
                return Self::Verifying;
            }
            return Self::Working;
        }
        if matches!(app.runtime_turn_status.as_deref(), Some("completed")) {
            return Self::Done;
        }
        if !app.input.is_empty() {
            return Self::Typing;
        }
        Self::Idle
    }

    #[must_use]
    pub fn label(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Idle => tr(locale, MessageId::PhaseIdle),
            Self::Typing => tr(locale, MessageId::PhaseDraft),
            Self::Working => tr(locale, MessageId::PhaseWorking),
            Self::Verifying => tr(locale, MessageId::PhaseVerifying),
            Self::Waiting | Self::Approval => tr(locale, MessageId::PhaseWaitingOnYou),
            Self::Done => tr(locale, MessageId::PhaseDone),
            Self::Failed => tr(locale, MessageId::PhaseFailed),
        }
    }

    #[must_use]
    pub fn color(self, app: &App) -> Color {
        match self {
            Self::Idle => app.ui_theme.text_muted,
            Self::Done => app.ui_theme.success,
            Self::Typing => app.ui_theme.accent_primary,
            // Verifying shares the live seafoam hue; the tick-vs-bubble
            // marker carries the checking/searching distinction.
            Self::Working | Self::Verifying => app.ui_theme.status_working,
            Self::Waiting | Self::Approval => app.ui_theme.accent_action,
            Self::Failed => app.ui_theme.error_fg,
        }
    }
}

/// Summarize only tools whose lifecycle is actually `Running`. A read label
/// is earned only when every running entry is read/exploration-shaped; mixed
/// work stays the neutral `using tool`. Verification wins because it is the
/// existing stronger promise made by the phase strip.
fn running_tool_facts(app: &App) -> RunningToolFacts {
    use crate::tui::history::{HistoryCell, ToolCell, ToolStatus};
    use crate::tui::widgets::tool_card::{ToolFamily, tool_family_for_name};

    let mut facts = RunningToolFacts::default();
    let Some(active) = app.active_cell.as_ref() else {
        return facts;
    };
    for cell in active.entries() {
        let HistoryCell::Tool(tool) = cell else {
            continue;
        };
        match tool {
            ToolCell::Exec(exec) if exec.status == ToolStatus::Running => {
                facts.observe(false, exec_is_verification(&exec.command));
            }
            ToolCell::Generic(generic) if generic.status == ToolStatus::Running => {
                let family = tool_family_for_name(&generic.name);
                facts.observe(
                    matches!(family, ToolFamily::Read | ToolFamily::Find),
                    family == ToolFamily::Verify || generic.name == "read_lints",
                );
            }
            ToolCell::Exploring(exploring) => {
                for entry in &exploring.entries {
                    if entry.status == ToolStatus::Running {
                        facts.observe(true, false);
                    }
                }
            }
            ToolCell::WebSearch(search) if search.status == ToolStatus::Running => {
                facts.observe(true, false);
            }
            other if other.status() == Some(ToolStatus::Running) => {
                facts.observe(false, false);
            }
            _ => {}
        }
    }
    facts
}

fn exec_is_verification(command: &str) -> bool {
    let trimmed = command.trim_start();
    let mut tokens = trimmed.split_whitespace();
    let first = tokens.next().unwrap_or("");
    let second = tokens.next().unwrap_or("");
    match first {
        "cargo" => matches!(second, "test" | "check" | "clippy" | "nextest"),
        "go" => matches!(second, "test" | "vet"),
        "npm" | "pnpm" | "yarn" | "bun" => matches!(second, "test" | "lint" | "check"),
        "make" => matches!(second, "test" | "check" | "lint"),
        "python" | "python3" => trimmed.contains("-m pytest") || trimmed.contains("-m unittest"),
        "pytest" | "jest" | "vitest" | "tsc" | "eslint" | "ruff" | "mypy" | "clippy-driver"
        | "golangci-lint" | "shellcheck" => true,
        _ => false,
    }
}

fn completion_elapsed_ms(app: &App) -> Option<u128> {
    if !app.motion_policy().allows_decorative() {
        return None;
    }
    app.ocean_completion_started_at
        .map(|started| started.elapsed().as_millis())
        .filter(|elapsed| *elapsed < COMPLETION_BREATH_MS)
}

#[cfg(test)]
pub(crate) fn phase_marker(app: &App, phase: ShellPhase) -> (&'static str, Cow<'static, str>) {
    phase_marker_with_activity(app, phase, LiveActivity::from_app(app))
}

pub(crate) fn phase_marker_with_activity(
    app: &App,
    phase: ShellPhase,
    activity: LiveActivity,
) -> (&'static str, Cow<'static, str>) {
    let locale = app.ui_locale;
    match phase {
        ShellPhase::Idle => ("·", phase.label(locale)),
        ShellPhase::Typing => ("›", phase.label(locale)),
        ShellPhase::Working => {
            // The footer and the live tool card share one wall-clock cadence,
            // so the two primary liveness marks never look like unrelated
            // spinners. The shared helper also preserves the 400ms
            // "motion is earned" delay and reduced/still fallback.
            let policy = app.motion_policy();
            let animated = crate::tui::spinner::braille_spinner_frame(app.turn_started_at, false);
            let earned = app.turn_started_at.is_none_or(|started| {
                started.elapsed().as_millis()
                    >= u128::from(crate::tui::spinner::LIVE_MARKER_DELAY_MS)
            });
            let frame = policy.spinner_glyph(animated, earned);
            (frame, activity.label(locale))
        }
        ShellPhase::Verifying => {
            // Metered braille tick on the shared live clock — checking, not
            // searching. Reduced motion holds the legible mid frame.
            let policy = app.motion_policy();
            let animated = crate::tui::spinner::verification_tick_frame(app.turn_started_at, false);
            let earned = app.turn_started_at.is_none_or(|started| {
                started.elapsed().as_millis()
                    >= u128::from(crate::tui::spinner::LIVE_MARKER_DELAY_MS)
            });
            let frame = policy.spinner_glyph(animated, earned);
            (frame, phase.label(locale))
        }
        ShellPhase::Waiting | ShellPhase::Approval => ("◆", phase.label(locale)),
        ShellPhase::Done => match completion_elapsed_ms(app) {
            Some(elapsed) if elapsed < COMPLETION_RELEASE_MS => {
                let index = ((elapsed / 140) as usize + 4).min(WORKING_BUBBLE_FRAMES.len() - 1);
                (
                    WORKING_BUBBLE_FRAMES[index],
                    tr(locale, MessageId::PhaseFinishing),
                )
            }
            _ => (crate::tui::glyphs::DONE, phase.label(locale)),
        },
        ShellPhase::Failed => (crate::tui::glyphs::FAILED, phase.label(locale)),
    }
}

fn mode_label(locale: Locale, mode: AppMode) -> Cow<'static, str> {
    match mode {
        AppMode::Agent | AppMode::Auto | AppMode::Yolo => tr(locale, MessageId::ChipModeAct),
        AppMode::Plan => tr(locale, MessageId::ChipModePlan),
        AppMode::Operate => tr(locale, MessageId::ChipModeOperate),
    }
}

/// Permission chip words. This maps from the typed [`ApprovalMode`] state —
/// never from the English `permission_chip_label()` strings — so localizing
/// (or rewording) the upstream chip labels can never silently break the chip.
fn permission_label(app: &App) -> Cow<'static, str> {
    let locale = app.ui_locale;
    if app.mode == AppMode::Plan {
        return tr(locale, MessageId::ChipPermissionReadOnly);
    }
    match app.approval_mode {
        ApprovalMode::Suggest => tr(locale, MessageId::ChipPermissionAsk),
        ApprovalMode::Auto => tr(locale, MessageId::ChipPermissionAuto),
        // Keep the effective permission explicit. `bypass` is an
        // implementation detail and, more importantly, can imply that
        // repository law no longer applies. Full Access never bypasses
        // constitution rules.
        ApprovalMode::Bypass => tr(locale, MessageId::ChipPermissionFullAccess),
        ApprovalMode::Never => tr(locale, MessageId::ChipPermissionNever),
    }
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        result.push(ch);
        used += ch_width;
    }
    result.push('…');
    result
}

fn render_launch_line(area: Rect, buf: &mut Buffer, y: u16, spans: Vec<Span<'static>>) {
    if y >= area.height {
        return;
    }
    Paragraph::new(Line::from(spans)).render(
        Rect {
            x: area.x,
            y: area.y.saturating_add(y),
            width: area.width,
            height: 1,
        },
        buf,
    );
}

/// Render the distinct pre-session choice state. This screen contains no
/// transcript, composer, dashboard, or post-launch whale: each row dispatches
/// to real session/worktree machinery before the idle ocean is entered.
pub fn render_launch_screen(area: Rect, buf: &mut Buffer, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    Block::default()
        .style(Style::default().bg(app.ui_theme.surface_bg))
        .render(area, buf);
    let width = usize::from(area.width);
    let version = format!("v{}", env!("DEEPSEEK_BUILD_VERSION"));
    let workspace_budget = width.saturating_sub(version.width() + 6);
    let workspace = truncate_to_width(
        &crate::utils::display_path(&app.workspace),
        workspace_budget,
    );
    let mut header = vec![
        Span::styled(
            "cw",
            Style::default()
                .fg(app.ui_theme.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(workspace, Style::default().fg(app.ui_theme.text_muted)),
    ];
    let gap = width.saturating_sub(span_width(&header) + version.width());
    header.push(Span::raw(" ".repeat(gap)));
    header.push(Span::styled(
        version,
        Style::default().fg(app.ui_theme.text_hint),
    ));
    render_launch_line(area, buf, 0, header);
    if area.height > 1 {
        render_launch_line(
            area,
            buf,
            1,
            vec![Span::styled(
                "─".repeat(width),
                Style::default().fg(app.ui_theme.border),
            )],
        );
    }

    let rows_start = if area.height >= 16 { 4 } else { 3 };
    for (index, (label_id, key)) in LAUNCH_ROWS.iter().enumerate() {
        let y = rows_start + u16::try_from(index).unwrap_or(0);
        if y >= area.height.saturating_sub(3) {
            break;
        }
        let selected = app.launch.selected == index;
        let mut label = tr(app.ui_locale, *label_id).into_owned();
        if index == 1 && !app.launch.worktree_available {
            label.push_str(&format!(
                " · {}",
                tr(app.ui_locale, MessageId::LaunchMenuUnavailable)
            ));
        }
        if index == 2 {
            label.push_str(&format!(
                " · {}",
                tr(app.ui_locale, MessageId::LaunchMenuSavedCount)
                    .replace("{count}", &app.launch.workspace_session_count.to_string())
            ));
        }
        let prefix = if selected { "  ▸ " } else { "    " };
        let key_width = key.width();
        let label_budget = width.saturating_sub(prefix.width() + key_width + 2);
        let label = truncate_to_width(&label, label_budget);
        let fill = width.saturating_sub(prefix.width() + label.width() + key_width);
        let row_style = if selected {
            Style::default()
                .fg(app.ui_theme.accent_primary)
                .add_modifier(Modifier::BOLD)
        } else if index == 1 && !app.launch.worktree_available {
            Style::default().fg(app.ui_theme.text_dim)
        } else {
            Style::default().fg(app.ui_theme.text_body)
        };
        render_launch_line(
            area,
            buf,
            y,
            vec![
                Span::styled(prefix, row_style),
                Span::styled(label, row_style),
                Span::raw(" ".repeat(fill)),
                Span::styled(*key, Style::default().fg(app.ui_theme.text_hint)),
            ],
        );
    }

    if area.height < 3 {
        return;
    }
    let rule_y = area.height.saturating_sub(3);
    render_launch_line(
        area,
        buf,
        rule_y,
        vec![Span::styled(
            "─".repeat(width),
            Style::default().fg(app.ui_theme.border),
        )],
    );
    let prompt = if let Some(input) = app.launch.worktree_input.as_deref() {
        format!(
            "{}  {}{}",
            tr(app.ui_locale, MessageId::LaunchWorktreeNameLabel),
            input,
            if app.low_motion { "_" } else { "▌" }
        )
    } else if let Some(status) = app.launch.status.as_deref() {
        status.to_string()
    } else if area.width < 60 {
        format!(
            "j/k:{} · Enter:{}",
            tr(app.ui_locale, MessageId::LaunchHintMove),
            tr(app.ui_locale, MessageId::LaunchHintOpen)
        )
    } else {
        tr(app.ui_locale, MessageId::LaunchTipFlags).into_owned()
    };
    render_launch_line(
        area,
        buf,
        area.height.saturating_sub(2),
        vec![Span::styled(
            truncate_to_width(&prompt, width),
            Style::default().fg(if app.launch.status.is_some() {
                app.ui_theme.text_muted
            } else {
                app.ui_theme.text_hint
            }),
        )],
    );

    let saved_sessions = if app.launch.workspace_session_count == 1 {
        tr(app.ui_locale, MessageId::LaunchSavedSessionSingular).into_owned()
    } else {
        tr(app.ui_locale, MessageId::LaunchSavedSessionsPlural)
            .replace("{count}", &app.launch.workspace_session_count.to_string())
    };
    let status = format!(
        "{} · {} · {}",
        app.model_display_label(),
        mode_label(app.ui_locale, app.mode),
        saved_sessions
    );
    render_launch_line(
        area,
        buf,
        area.height.saturating_sub(1),
        vec![Span::styled(
            truncate_to_width(&status, width),
            Style::default().fg(app.ui_theme.text_dim),
        )],
    );
}

/// Record the launch row rects immediately after the launch frame is painted.
/// The coordinates mirror the renderer's responsive row placement exactly.
pub fn record_launch_row_areas(area: Rect, launch: &mut crate::tui::app::LaunchState) {
    launch.row_areas.clear();
    let rows_start = if area.height >= 16 { 4 } else { 3 };
    for index in 0..LAUNCH_ROWS.len() {
        let y = rows_start + u16::try_from(index).unwrap_or(0);
        if y >= area.height.saturating_sub(3) {
            break;
        }
        launch.row_areas.push(Rect {
            x: area.x,
            y: area.y.saturating_add(y),
            width: area.width,
            height: 1,
        });
    }
}

fn compact_tokens(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn compact_effort_label(label: &str) -> &'static str {
    let effective = label
        .rsplit_once('→')
        .map_or(label, |(_, effective)| effective);
    let effective = effective
        .rsplit_once(':')
        .map_or(effective, |(_, effective)| effective)
        .trim()
        .to_ascii_lowercase();
    match effective.as_str() {
        "off" => "o",
        "low" => "l",
        "med" | "medium" => "m",
        "high" => "h",
        "max" | "maximum" | "xhigh" => "x",
        "auto" => "a",
        _ => "·",
    }
}

fn session_token_breakdown(app: &App) -> Option<Span<'static>> {
    app.header_items.contains(&HeaderItem::Tokens).then(|| {
        Span::styled(
            format!(
                "{} in · {} cch · {} out",
                format_token_count_compact(u64::from(app.session.total_input_tokens)),
                format_token_count_compact(u64::from(app.session.total_cache_hit_tokens)),
                format_token_count_compact(u64::from(app.session.total_output_tokens)),
            ),
            Style::default().fg(app.ui_theme.info),
        )
    })
}

/// Render the one-line shell header. Route, mode, requested/effective effort,
/// permission, active-agent count, and context each have exactly one owner.
pub fn render_header(area: Rect, buf: &mut Buffer, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let tier = ShellTier::for_chrome_width(area.width);
    Block::default()
        .style(Style::default().bg(app.ui_theme.header_bg))
        .render(area, buf);

    let (effective_provider, effective_model) = app.effective_route_identity_display();
    let route_label = format!("{effective_provider} · {effective_model}");
    let effort_label = app.reasoning_effort_display_label();
    let mode_color = match app.mode {
        AppMode::Plan => app.ui_theme.mode_plan,
        AppMode::Operate => app.ui_theme.mode_operate,
        _ => app.ui_theme.mode_agent,
    };
    // Match the composer's warm top edge exactly: Ask amber, Auto-Review
    // Signal Gold, and Full Access coral.
    let permission_color = match app.approval_mode {
        ApprovalMode::Suggest | ApprovalMode::Never => app.ui_theme.permission_ask,
        ApprovalMode::Auto => app.ui_theme.permission_auto_review,
        ApprovalMode::Bypass => app.ui_theme.permission_full_access,
    };
    let status_indicator = crate::tui::widgets::header_status_indicator_frame(
        (!app.low_motion && app.fancy_animations)
            .then_some(app.turn_started_at)
            .flatten(),
        &app.status_indicator,
    )
    .filter(|indicator| *indicator != "cw");
    let mut left = vec![
        Span::styled(
            "cw",
            Style::default()
                .fg(app.ui_theme.accent_primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(route_label, Style::default().fg(app.ui_theme.text_muted)),
        Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
        Span::styled(
            mode_label(app.ui_locale, app.mode),
            Style::default().fg(mode_color),
        ),
        Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
        Span::styled(effort_label.clone(), Style::default().fg(app.ui_theme.info)),
    ];
    // The selected brand/status mark is part of the user's chosen header,
    // not expendable wide-screen decoration. Keep it in compact layouts too;
    // route text truncates before the permission posture or selected mark.
    if let Some(indicator) = status_indicator {
        left.push(Span::raw(" "));
        left.push(Span::styled(
            indicator,
            Style::default()
                .fg(app.ui_theme.info)
                .add_modifier(Modifier::BOLD),
        ));
    }
    // Permission is safety state, not optional chrome. Compact terminals shed
    // route detail and the context meter, but keep mode, effective effort, and
    // the effective posture.
    left.push(Span::styled(
        " · ",
        Style::default().fg(app.ui_theme.text_dim),
    ));
    left.push(Span::styled(
        permission_label(app),
        Style::default().fg(permission_color),
    ));

    let context_meter = (tier != ShellTier::Compact)
        .then(|| crate::tui::ui::context_usage_snapshot(app))
        .flatten()
        .map(|(used, max, percent)| {
            let filled = ((percent / 100.0) * 5.0).ceil().clamp(0.0, 5.0) as usize;
            Span::styled(
                format!(
                    "{}/{} [{}{}] {:.0}%",
                    compact_tokens(used),
                    compact_tokens(i64::from(max)),
                    "▰".repeat(filled),
                    "▱".repeat(5usize.saturating_sub(filled)),
                    percent
                ),
                Style::default().fg(app.ui_theme.info),
            )
        });
    let token_breakdown = (tier != ShellTier::Compact)
        .then(|| session_token_breakdown(app))
        .flatten();
    let version = (tier == ShellTier::Wide).then(|| {
        Span::styled(
            format!("v{}", env!("DEEPSEEK_BUILD_VERSION")),
            Style::default().fg(app.ui_theme.text_hint),
        )
    });
    // Build the existing right-hand chrome exactly as before. Configured
    // header items may use spare width, but falling back must restore this
    // baseline rather than eliding context or version information.
    let mut right = Vec::new();
    if let Some(context_meter) = context_meter {
        right.push(context_meter);
    }
    if let Some(version) = version {
        if !right.is_empty() {
            right.push(Span::raw("  "));
        }
        right.push(version);
    }

    let minimum_effort = if tier == ShellTier::Compact {
        compact_effort_label(&effort_label).to_string()
    } else {
        effort_label.clone()
    };
    let indicator_width = status_indicator.map_or(0, |indicator| 1 + indicator.width());
    let minimum_left_width = 4usize
        .saturating_add(indicator_width)
        .saturating_add(3 + mode_label(app.ui_locale, app.mode).width())
        .saturating_add(3 + minimum_effort.width())
        .saturating_add(3 + permission_label(app).width());
    let available = usize::from(area.width);
    if let Some(token_breakdown) = token_breakdown {
        let mut enhanced_right = vec![token_breakdown];
        if !right.is_empty() {
            enhanced_right.push(Span::raw("  "));
            enhanced_right.extend(right.iter().cloned());
        }
        let enhanced_width = span_width(&enhanced_right);
        let gap = usize::from(enhanced_width > 0);
        if minimum_left_width
            .saturating_add(gap)
            .saturating_add(enhanced_width)
            <= available
        {
            right = enhanced_right;
        }
    }

    let right_width = span_width(&right);
    let left_budget = available.saturating_sub(right_width + usize::from(right_width > 0));
    if span_width(&left) > left_budget {
        let mode = mode_label(app.ui_locale, app.mode);
        let permission = permission_label(app);
        let effort = if tier == ShellTier::Compact {
            compact_effort_label(&effort_label).to_string()
        } else {
            effort_label.clone()
        };
        let suffix = vec![
            Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
            Span::styled(mode, Style::default().fg(mode_color)),
            Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
            Span::styled(effort, Style::default().fg(app.ui_theme.info)),
            Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
            Span::styled(permission, Style::default().fg(permission_color)),
        ];
        let indicator_width = status_indicator.map_or(0, |indicator| 1 + indicator.width());
        let fixed_width = 4usize
            .saturating_add(indicator_width)
            .saturating_add(span_width(&suffix));
        let model_budget = left_budget.saturating_sub(fixed_width);
        left = vec![Span::styled(
            "cw",
            Style::default()
                .fg(app.ui_theme.accent_primary)
                .add_modifier(Modifier::BOLD),
        )];
        if let Some(indicator) = status_indicator {
            left.push(Span::raw(" "));
            left.push(Span::styled(
                indicator,
                Style::default()
                    .fg(app.ui_theme.info)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        left.push(Span::raw("  "));
        left.push(Span::styled(
            truncate_to_width(&app.model_display_label(), model_budget),
            Style::default().fg(app.ui_theme.text_muted),
        ));
        left.extend(suffix);
    }
    let left_width = span_width(&left);
    let gap = available.saturating_sub(left_width + right_width);
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    let title_area = Rect { height: 1, ..area };
    Paragraph::new(Line::from(left)).render(title_area, buf);
    if area.height > 1 {
        let rule_area = Rect {
            y: area.y.saturating_add(1),
            height: 1,
            ..area
        };
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(usize::from(area.width)),
            Style::default().fg(app.ui_theme.border),
        )))
        .render(rule_area, buf);
    }
}

/// Render the fixed one-line phase band.
///
/// Ocean placement (above vs below the composer) is owned by
/// [`crate::tui::phase_strip`]; this entry point only paints the band so
/// classic callers and tests keep a stable name.
pub fn render_footer(area: Rect, buf: &mut Buffer, app: &mut App) {
    crate::tui::phase_strip::render(area, buf, app);
}

/// Build the post-launch idle composition. It is deliberately not a command
/// dashboard: one brand mark, one context line, and one quiet Fleet setup path.
#[must_use]
pub(crate) fn empty_state_mark_visible(area: Rect) -> bool {
    ShellTier::for_area(area) != ShellTier::Compact && area.height >= 14 && area.width >= 28
}

#[must_use]
pub(crate) fn decorative_shell_motion_enabled(app: &App) -> bool {
    app.motion_policy().allows_decorative()
        && app.ocean_treatment.supports_ambient_life()
        && !app.attention_hold_active()
        && app.onboarding == OnboardingState::None
        && !app.launch.visible
        && app.view_stack.is_empty()
}

#[must_use]
fn idle_mark_animation_enabled(app: &App) -> bool {
    decorative_shell_motion_enabled(app) && matches!(ShellPhase::from_app(app), ShellPhase::Idle)
}

/// Raised-cosine caustic band for the idle whale. The 4s cycle spends roughly
/// 1.3s crossing the mark and parks off-screen for the remainder, so the brand
/// has a clear moment of life without becoming looping chrome.
#[must_use]
fn idle_mark_shine_opacity(diagonal: f32, elapsed_ms: u128) -> f32 {
    let cycle_progress = (elapsed_ms % IDLE_SHIMMER_CYCLE_MS) as f32 / IDLE_SHIMMER_CYCLE_MS as f32;
    let sweep_progress = (cycle_progress / IDLE_SHIMMER_SWEEP_FRACTION).min(1.0);
    let band_position =
        -IDLE_SHIMMER_BAND_HALF_WIDTH + sweep_progress * (1.0 + 2.0 * IDLE_SHIMMER_BAND_HALF_WIDTH);
    let distance = (diagonal - band_position).abs();
    if distance >= IDLE_SHIMMER_BAND_HALF_WIDTH {
        return 0.0;
    }
    let raised_cosine =
        0.5 * (1.0 + (std::f32::consts::PI * distance / IDLE_SHIMMER_BAND_HALF_WIDTH).cos());
    IDLE_SHIMMER_STRENGTH * raised_cosine
}

#[must_use]
fn idle_mark_color(base: Color, highlight: Color, opacity: f32) -> Color {
    if opacity <= 0.0 {
        return base;
    }
    match (base, highlight) {
        (Color::Rgb(..), Color::Rgb(..)) => crate::palette::blend(highlight, base, opacity),
        // Named/terminal-owned colors cannot be blended truthfully. Hold the
        // stable brand color instead of flashing the entire mark at full ink.
        _ => base,
    }
}

fn idle_whale_is_uwu(app: &App) -> bool {
    app.ui_theme.name == "uwu"
}

fn idle_whale_spout_row(app: &App) -> &'static str {
    if idle_whale_is_uwu(app) {
        UWU_IDLE_WHALE_SPOUT_ROW
    } else {
        IDLE_WHALE_SPOUT_ROW
    }
}

fn idle_whale_rows(app: &App) -> [&'static str; 3] {
    if idle_whale_is_uwu(app) {
        UWU_IDLE_WHALE_ROWS
    } else {
        IDLE_WHALE_ROWS
    }
}

fn idle_whale_row_spans(
    text: &'static str,
    row: usize,
    elapsed_ms: u128,
    animated: bool,
    base: Color,
    highlight: Color,
    eye: Color,
) -> Vec<Span<'static>> {
    let rows = IDLE_WHALE_ROWS.len() as f32;
    let cols = IDLE_WHALE_ROWS
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(1) as f32;
    let mut spans = Vec::new();
    let mut run = String::new();
    let mut run_color = None;

    for (column, ch) in text.chars().enumerate() {
        let diagonal = (column as f32 + (rows - 1.0 - row as f32)) / (cols + rows);
        let color = if matches!(ch, '·' | '░' | '✦') {
            // Soft uwu blush/sparkle use the eye/sakura channel; classic only has ·.
            eye
        } else if animated {
            idle_mark_color(
                base,
                highlight,
                idle_mark_shine_opacity(diagonal, elapsed_ms),
            )
        } else {
            base
        };
        if run_color != Some(color) {
            if let Some(previous) = run_color {
                spans.push(Span::styled(
                    std::mem::take(&mut run),
                    Style::default().fg(previous),
                ));
            }
            run_color = Some(color);
        }
        run.push(ch);
    }
    if let Some(previous) = run_color {
        spans.push(Span::styled(run, Style::default().fg(previous)));
    }
    spans
}

#[must_use]
fn idle_whale_block_width() -> usize {
    let classic = std::iter::once(IDLE_WHALE_SPOUT_ROW)
        .chain(IDLE_WHALE_ROWS.iter().copied())
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    let uwu = std::iter::once(UWU_IDLE_WHALE_SPOUT_ROW)
        .chain(UWU_IDLE_WHALE_ROWS.iter().copied())
        .map(UnicodeWidthStr::width)
        .max()
        .unwrap_or(0);
    classic.max(uwu)
}

pub fn empty_state_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let width = usize::from(area.width);
    let tier = ShellTier::for_area(area);
    let mut lines = vec![Line::from(""); usize::from(area.height / 4)];
    if empty_state_mark_visible(area) {
        let animated = idle_mark_animation_enabled(app);
        let elapsed_ms = app.ocean_started_at.elapsed().as_millis();
        let spout = idle_whale_spout_row(app);
        let rows = idle_whale_rows(app);
        let mut mark = vec![vec![Span::styled(
            spout,
            Style::default().fg(app.ui_theme.accent_secondary),
        )]];
        // Soft uwu: sakura blush/sparkle glyphs; classic keeps body peach + text eye.
        let highlight = if idle_whale_is_uwu(app) {
            app.ui_theme.accent_primary
        } else {
            app.ui_theme.text_body
        };
        mark.extend(rows.iter().enumerate().map(|(row, text)| {
            idle_whale_row_spans(
                text,
                row,
                elapsed_ms,
                animated,
                app.ui_theme.accent_action,
                app.ui_theme.text_body,
                highlight,
            )
        }));
        // The spout, head, belly, peduncle, and flukes are one drawing. Give
        // every row the same outer inset so the authored offsets survive;
        // centering each row independently shears the silhouette apart.
        let block_inset = " ".repeat(width.saturating_sub(idle_whale_block_width()) / 2);
        for row in mark {
            let mut spans = vec![Span::raw(block_inset.clone())];
            spans.extend(row);
            lines.push(Line::from(spans));
        }
        lines.push(Line::from(""));
    }

    let identity = crate::tui::workspace_context::identity_from_context(
        &app.workspace,
        app.workspace_context.as_deref(),
    );
    let workspace = crate::utils::display_path(&app.workspace);
    let branch = identity.branch.as_deref().map_or_else(
        || tr(app.ui_locale, MessageId::EmptyStateNoGit),
        |branch| Cow::Owned(branch.to_string()),
    );
    let context = if tier == ShellTier::Compact {
        format!("codewhale · {branch}")
    } else {
        format!(
            "codewhale · {workspace} · {branch} · {} {}",
            tr(app.ui_locale, MessageId::EmptyStateMcpLabel),
            app.mcp_configured_count
        )
    };
    let context = truncate_to_width(&context, width);
    let inset = " ".repeat(width.saturating_sub(context.width()) / 2);
    lines.push(Line::from(Span::styled(
        format!("{inset}{context}"),
        Style::default().fg(app.ui_theme.text_muted),
    )));
    if area.height >= 6 {
        lines.push(Line::from(""));
        let (fleet_label, fleet_action) = if app.onboarding_needs_api_key {
            // `--skip-onboarding` can expose the launch shell without a usable
            // provider route. Do not claim that Fleet is ready in that state;
            // point at the boundary that can actually make it runnable.
            (
                tr(app.ui_locale, MessageId::EmptyStateFleetLabel),
                "/provider",
            )
        } else {
            // Built-in roles are immediately usable with the active route.
            // Keep this truth at every responsive tier so `/fleet setup`
            // reads as optional customization instead of required setup.
            (
                tr(app.ui_locale, MessageId::EmptyStateFleetSetupLabel),
                "/fleet setup",
            )
        };
        let fleet = format!("{fleet_label}  {fleet_action}");
        let inset = " ".repeat(width.saturating_sub(fleet.width()) / 2);
        lines.push(Line::from(Span::styled(
            format!("{inset}{fleet}"),
            Style::default().fg(app.ui_theme.text_hint),
        )));
        if area.height >= 7 {
            let help = format!(
                "/help or Ctrl+K {}",
                tr(app.ui_locale, MessageId::EmptyStateHelpHint)
            );
            let inset = " ".repeat(width.saturating_sub(help.width()) / 2);
            lines.push(Line::from(Span::styled(
                format!("{inset}{help}"),
                Style::default().fg(app.ui_theme.text_hint),
            )));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        tui::app::{LaunchState, TuiOptions},
    };
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn test_app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-flash".to_string(),
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
                start_in_agent_mode: true,
                skip_onboarding: true,
                yolo: false,
                resume_session_id: None,
                initial_input: None,
            },
            &Config::default(),
        )
    }

    fn launch() -> LaunchState {
        LaunchState {
            visible: true,
            selected: 0,
            worktree_input: None,
            status: None,
            workspace_session_count: 2,
            worktree_available: true,
            row_areas: Vec::new(),
        }
    }

    #[test]
    fn launch_row_hitboxes_follow_responsive_render_rows() {
        let mut launch = launch();
        record_launch_row_areas(Rect::new(3, 2, 80, 24), &mut launch);
        assert_eq!(launch.row_areas.len(), 5);
        assert_eq!(launch.row_areas[0], Rect::new(3, 6, 80, 1));
        assert_eq!(launch.row_areas[4], Rect::new(3, 10, 80, 1));

        record_launch_row_areas(Rect::new(3, 2, 40, 10), &mut launch);
        assert_eq!(launch.row_areas.len(), 4);
        assert_eq!(launch.row_areas[0], Rect::new(3, 5, 40, 1));
    }

    fn footer_text(app: &mut App) -> String {
        let area = Rect::new(0, 0, 100, 1);
        let mut buf = Buffer::empty(area);
        render_footer(area, &mut buf, app);
        (0..area.width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    fn header_text(app: &App, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        render_header(area, &mut buf, app);
        (0..width).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[test]
    fn configured_session_tokens_follow_underwater_header_width_priority() {
        let mut app = test_app();
        app.header_items = vec![HeaderItem::Tokens];
        app.session.total_input_tokens = 18_000;
        app.session.total_cache_hit_tokens = 12_000;
        app.session.total_output_tokens = 6_000;
        app.session.last_prompt_tokens = Some(48_000);

        for (width, should_show_tokens, should_show_context) in [
            (40, false, false),
            (60, false, true),
            (80, true, true),
            (100, true, true),
        ] {
            let header = header_text(&app, width);
            assert_eq!(
                header.contains("18.0k in · 12.0k cch · 6.0k out"),
                should_show_tokens,
                "unexpected token visibility at width {width}: {header:?}",
            );
            assert_eq!(
                header.contains('%'),
                should_show_context,
                "unexpected context visibility at width {width}: {header:?}",
            );
            assert!(
                header.to_ascii_lowercase().contains("act"),
                "mode must survive at width {width}: {header:?}",
            );
            assert!(
                header.to_ascii_lowercase().contains("ask"),
                "permission must survive at width {width}: {header:?}",
            );
        }
    }

    #[test]
    fn underwater_header_keeps_session_tokens_opt_in() {
        let mut app = test_app();
        app.header_items.clear();
        app.session.total_input_tokens = 18_000;
        app.session.total_cache_hit_tokens = 12_000;
        app.session.total_output_tokens = 6_000;
        app.session.last_prompt_tokens = Some(48_000);

        let normal_header = header_text(&app, 60);
        let wide_header = header_text(&app, 110);

        assert!(
            !normal_header.contains("18.0k in"),
            "header: {normal_header:?}"
        );
        assert!(
            normal_header.contains('%'),
            "context meter missing: {normal_header:?}"
        );
        assert!(
            wide_header.contains('%'),
            "context meter missing: {wide_header:?}"
        );
        assert!(
            wide_header.contains(&format!("v{}", env!("DEEPSEEK_BUILD_VERSION"))),
            "version missing: {wide_header:?}"
        );
    }

    #[test]
    fn compact_header_keeps_mode_and_effective_permission() {
        let mut app = test_app();
        app.mode = AppMode::Operate;
        app.approval_mode = ApprovalMode::Bypass;
        app.reasoning_effort = crate::tui::app::ReasoningEffort::Low;
        app.model = "provider/model-with-a-deliberately-long-route-name".to_string();

        let header = header_text(&app, 40);

        assert!(header.starts_with("cw"), "brand missing: {header:?}");
        assert!(
            header.to_ascii_lowercase().contains("operate"),
            "mode missing: {header:?}"
        );
        assert!(
            header.contains("Full Access"),
            "permission posture missing: {header:?}"
        );
        assert!(
            header.contains(" · h · Full Access"),
            "effective effort missing: {header:?}"
        );
    }

    #[test]
    fn header_labels_follow_the_ask_amber_auto_gold_full_access_coral_ramp() {
        for width in [40, 100] {
            for (approval_mode, expected_label) in [
                (ApprovalMode::Suggest, "ask"),
                (ApprovalMode::Auto, "auto"),
                (ApprovalMode::Bypass, "Full Access"),
            ] {
                let mut app = test_app();
                app.approval_mode = approval_mode;
                let expected_color = match approval_mode {
                    ApprovalMode::Suggest | ApprovalMode::Never => app.ui_theme.permission_ask,
                    ApprovalMode::Auto => app.ui_theme.permission_auto_review,
                    ApprovalMode::Bypass => app.ui_theme.permission_full_access,
                };
                let label = permission_label(&app).into_owned();
                assert_eq!(label, expected_label, "{approval_mode:?}");
                let area = Rect::new(0, 0, width, 1);
                let mut buf = Buffer::empty(area);

                render_header(area, &mut buf, &app);

                let rendered = (0..width).map(|x| buf[(x, 0)].symbol()).collect::<String>();
                // `auto` can also appear earlier as a route/mode label. The
                // permission posture owns the rightmost occurrence.
                let label_byte = rendered
                    .rfind(&label)
                    .expect("permission label should render");
                let label_x = rendered[..label_byte].width() as u16;
                assert_eq!(buf[(label_x, 0)].fg, expected_color, "{approval_mode:?}");
            }
        }
    }

    #[test]
    fn normal_header_keeps_requested_effective_effort_before_route_detail() {
        let mut app = test_app();
        app.mode = AppMode::Operate;
        app.approval_mode = ApprovalMode::Bypass;
        app.reasoning_effort = crate::tui::app::ReasoningEffort::Low;
        app.model = "provider/model-with-a-deliberately-long-route-name".to_string();

        let header = header_text(&app, 80);

        assert!(header.contains("low→high"), "effort missing: {header:?}");
        assert!(
            header.to_ascii_lowercase().contains("operate"),
            "mode missing: {header:?}"
        );
        assert!(
            header.contains("Full Access"),
            "permission posture missing: {header:?}"
        );
    }

    #[test]
    fn compact_header_keeps_the_selected_whale_indicator() {
        let mut app = test_app();
        app.status_indicator = "whale".to_string();
        app.model = "provider/model-with-a-deliberately-long-route-name".to_string();

        let header = header_text(&app, 40);

        assert!(header.contains('🐳'), "selected whale missing: {header:?}");
    }

    #[test]
    fn header_shows_exact_named_custom_provider() {
        let mut app = test_app();
        app.set_provider_identity(crate::config::ApiProvider::Custom, "lm-studio");
        app.model = "local-code-model".to_string();

        let header = header_text(&app, 100);

        assert!(
            header.contains("lm-studio · local-code-model"),
            "{header:?}"
        );
        assert!(!header.contains("Custom ·"), "{header:?}");
    }

    /// The footer consumes the toast system, not the legacy status sink: an
    /// informational acknowledgement must leave on its own instead of
    /// becoming permanent idle chrome.
    #[test]
    fn footer_notices_expire_instead_of_becoming_permanent_chrome() {
        let mut app = test_app();
        app.status_message = Some("Auto-compaction enabled".to_string());

        let fresh = footer_text(&mut app);
        assert!(
            fresh.contains("Auto-compaction enabled"),
            "a fresh notice should surface once: {fresh}"
        );

        for toast in &mut app.status_toasts {
            toast.created_at = Instant::now() - Duration::from_secs(60);
        }
        let later = footer_text(&mut app);
        assert!(
            !later.contains("Auto-compaction"),
            "an informational acknowledgement must expire without user action: {later}"
        );
        assert!(
            later.contains("idle"),
            "the stable phase fact survives the expiry: {later}"
        );
    }

    /// Errors are sticky: they outlive the informational TTL window and stay
    /// until their own resolution window passes.
    #[test]
    fn footer_errors_outlive_informational_acknowledgements() {
        let mut app = test_app();
        app.status_message = Some("Provider request failed: timeout".to_string());

        let fresh = footer_text(&mut app);
        assert!(fresh.contains("failed"), "error notice missing: {fresh}");

        if let Some(sticky) = app.sticky_status.as_mut() {
            sticky.created_at = Instant::now() - Duration::from_secs(6);
        } else {
            panic!("an error must be promoted to the sticky slot");
        }
        let held = footer_text(&mut app);
        assert!(
            held.contains("failed"),
            "errors must hold past the informational window: {held}"
        );
    }

    #[test]
    fn launch_rows_and_direct_keys_share_actions() {
        let mut state = launch();
        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::NewSession
        );
        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::Resume
        );
        assert_eq!(state.selected, 2);

        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::Changelog
        );
        assert_eq!(state.selected, 3);
    }

    #[test]
    fn worktree_action_collects_a_name_before_creation() {
        let mut state = launch();
        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::None
        );
        for ch in "repair-pty".chars() {
            assert_eq!(
                handle_launch_key(
                    &mut state,
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                    Locale::En,
                ),
                LaunchAction::None
            );
        }
        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::CreateWorktree("repair-pty".to_string())
        );
    }

    #[test]
    fn unavailable_worktree_is_truthful_and_non_destructive() {
        let mut state = launch();
        state.worktree_available = false;
        assert_eq!(
            handle_launch_key(
                &mut state,
                KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::None
        );
        assert!(state.worktree_input.is_none());
        assert_eq!(
            state.status.as_deref(),
            Some("New worktree requires a Git repository.")
        );
    }

    #[test]
    fn phase_markers_make_motion_and_attention_explicit() {
        let mut app = test_app();

        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_started_at = Some(Instant::now() - Duration::from_millis(1_300));
        let (working, label) = phase_marker(&app, ShellPhase::from_app(&app));
        assert!(crate::tui::spinner::BRAILLE_SPINNER_FRAMES.contains(&working));
        assert_eq!(label, "working");

        app.low_motion = true;
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(9));
        assert_eq!(
            phase_marker(&app, ShellPhase::Working).0,
            WORKING_BUBBLE_FRAMES[4]
        );

        app.runtime_turn_status = None;
        app.plan_prompt_pending = true;
        let (marker, label) = phase_marker(&app, ShellPhase::from_app(&app));
        assert_eq!(marker, "◆");
        assert_eq!(label, "waiting on you");

        app.plan_prompt_pending = false;
        app.runtime_turn_status = Some("failed".to_string());
        let (marker, label) = phase_marker(&app, ShellPhase::from_app(&app));
        assert_eq!(marker, "✕");
        assert_eq!(label, "failed");
    }

    #[test]
    fn live_activity_is_truthful_prioritized_and_ignores_stale_tools() {
        use crate::tui::active_cell::ActiveCell;
        use crate::tui::history::{
            ExploringCell, ExploringEntry, GenericToolCell, HistoryCell, ToolCell, ToolStatus,
        };

        let generic = |name: &str, status: ToolStatus| {
            HistoryCell::Tool(ToolCell::Generic(GenericToolCell {
                name: name.to_string(),
                status,
                input_summary: None,
                output: None,
                prompts: None,
                spillover_path: None,
                output_summary: None,
                is_diff: false,
            }))
        };
        let reading = || {
            HistoryCell::Tool(ToolCell::Exploring(ExploringCell {
                entries: vec![ExploringEntry {
                    label: "Reading src/lib.rs".to_string(),
                    status: ToolStatus::Running,
                }],
            }))
        };

        let mut app = test_app();

        // A completed tool may remain in the active group until TurnComplete,
        // but it cannot manufacture liveness on its own.
        let mut stale = ActiveCell::new();
        stale.push_tool("done", generic("write_file", ToolStatus::Success));
        app.active_cell = Some(stale);
        assert_eq!(
            LiveActivity::from_app(&app).kind(),
            LiveActivityKind::Working
        );
        assert_eq!(ShellPhase::from_app(&app), ShellPhase::Idle);

        // Only the explicit streaming pointer earns the reasoning label. No
        // configured effort, elapsed clock, or generic loading inference is
        // involved.
        let mut active = ActiveCell::new();
        let thinking = active.push_thinking(HistoryCell::Thinking {
            content: "private reasoning must not reach the strip".to_string(),
            streaming: true,
            duration_secs: None,
        });
        app.active_cell = Some(active);
        app.streaming_thinking_active_entry = Some(thinking);
        assert_eq!(
            LiveActivity::from_app(&app).kind(),
            LiveActivityKind::Reasoning
        );
        let (_, label) = phase_marker(&app, ShellPhase::from_app(&app));
        assert_eq!(label, "reasoning");
        assert!(!label.contains("private"));

        // A running read wins over a stale thinking pointer.
        app.active_cell
            .as_mut()
            .expect("active cell")
            .push_tool("read", reading());
        let activity = LiveActivity::from_app(&app);
        assert_eq!(activity.kind(), LiveActivityKind::Reading);
        assert_eq!(activity.running_tool_count(), 1);
        assert_eq!(phase_marker(&app, ShellPhase::Working).1, "reading");

        // Mixed tool work is not mislabeled as a pure read pass.
        app.active_cell
            .as_mut()
            .expect("active cell")
            .push_tool("write", generic("write_file", ToolStatus::Running));
        let activity = LiveActivity::from_app(&app);
        assert_eq!(activity.kind(), LiveActivityKind::UsingTool);
        assert_eq!(activity.running_tool_count(), 2);
        assert_eq!(phase_marker(&app, ShellPhase::Working).1, "using tool");

        // Verification remains the strongest live promise.
        app.active_cell
            .as_mut()
            .expect("active cell")
            .push_tool("verify", generic("run_verifiers", ToolStatus::Running));
        assert_eq!(
            LiveActivity::from_app(&app).kind(),
            LiveActivityKind::Verifying
        );
        assert_eq!(ShellPhase::from_app(&app), ShellPhase::Verifying);
    }

    #[test]
    fn live_activity_marker_freezes_for_reduced_or_still_and_has_ascii_fallback() {
        let mut app = test_app();
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(5));

        app.low_motion = true;
        let reduced = phase_marker(&app, ShellPhase::Working).0;
        assert_eq!(reduced, crate::tui::spinner::BRAILLE_SPINNER_STILL_FRAME);

        app.low_motion = false;
        app.fancy_animations = false;
        let fancy_off = phase_marker(&app, ShellPhase::Working).0;
        assert_eq!(fancy_off, crate::tui::spinner::LIVE_STATIC_MARKER);

        let mut cell = ratatui::buffer::Cell::default();
        cell.set_symbol(fancy_off);
        crate::tui::color_compat::adapt_cell_symbol_for_ascii(&mut cell);
        assert_eq!(cell.symbol(), ">");
        assert!(cell.symbol().is_ascii());
    }

    #[test]
    fn idle_whale_caustic_sweeps_then_parks_offscreen() {
        assert_eq!(idle_mark_shine_opacity(0.5, 0), 0.0);
        assert!(
            idle_mark_shine_opacity(0.5, 640) > 0.32,
            "the raised-cosine band should reach its peak near mid-sweep"
        );
        assert_eq!(
            idle_mark_shine_opacity(0.5, 2_000),
            0.0,
            "the caustic must rest offscreen between sweeps"
        );
    }

    #[test]
    fn idle_whale_caustic_preserves_text_width_and_has_a_static_fallback() {
        let base = Color::Rgb(246, 196, 83);
        let highlight = Color::Rgb(246, 242, 232);
        let text = IDLE_WHALE_ROWS[0];
        let moving = idle_whale_row_spans(text, 0, 640, true, base, highlight, highlight);
        let parked = idle_whale_row_spans(text, 0, 2_000, true, base, highlight, highlight);
        let frozen_a = idle_whale_row_spans(text, 0, 640, false, base, highlight, highlight);
        let frozen_b = idle_whale_row_spans(text, 0, 2_000, false, base, highlight, highlight);

        let content = |spans: &[Span<'_>]| {
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };
        let colors =
            |spans: &[Span<'_>]| spans.iter().map(|span| span.style.fg).collect::<Vec<_>>();

        for spans in [&moving, &parked, &frozen_a, &frozen_b] {
            assert_eq!(content(spans), text);
            assert_eq!(span_width(spans), text.width());
        }
        assert_ne!(colors(&moving), colors(&parked));
        assert_eq!(colors(&frozen_a), colors(&frozen_b));
    }

    #[test]
    fn idle_whale_rows_share_one_centered_block_without_losing_authored_offsets() {
        let mut app = test_app();
        app.ui_theme = crate::palette::ThemeId::Whale.ui_theme();
        app.low_motion = true;
        let width = 60usize;
        let rendered = empty_state_lines(&app, Rect::new(0, 0, width as u16, 16))
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let block_width = idle_whale_block_width();
        let block_inset = (width - block_width) / 2;

        assert_eq!(
            block_width, 23,
            "the final mark should stay quiet at 60 cols"
        );
        for row in std::iter::once(IDLE_WHALE_SPOUT_ROW).chain(IDLE_WHALE_ROWS) {
            let line = rendered
                .iter()
                .find(|line| line.trim_start() == row.trim_start())
                .unwrap_or_else(|| panic!("missing authored whale row {row:?}"));
            let rendered_inset = line.chars().take_while(|ch| *ch == ' ').count();
            let authored_inset = row.chars().take_while(|ch| *ch == ' ').count();

            assert_eq!(
                rendered_inset - authored_inset,
                block_inset,
                "row drifted out of the shared silhouette: {line:?}"
            );
            assert!(
                line.width() <= block_inset + block_width,
                "row escaped the centered mark block: {line:?}"
            );
        }
    }

    #[test]
    fn idle_whale_has_a_recognizable_ascii_safe_silhouette() {
        let ascii_row = |row: &str| {
            let mut rendered = String::new();
            for ch in row.chars() {
                let mut cell = ratatui::buffer::Cell::default();
                cell.set_symbol(&ch.to_string());
                crate::tui::color_compat::adapt_cell_symbol_for_ascii(&mut cell);
                rendered.push_str(cell.symbol());
            }
            rendered
        };
        let rows = std::iter::once(IDLE_WHALE_SPOUT_ROW)
            .chain(IDLE_WHALE_ROWS)
            .map(ascii_row)
            .collect::<Vec<_>>();

        assert_eq!(
            rows,
            [
                "   o",
                r"  .###########.      \/",
                " |##.############----/",
                "  .############.",
            ]
        );
        assert!(rows.iter().all(|row| row.is_ascii()));
    }

    #[test]
    fn reduced_motion_keeps_the_whole_idle_mark_still_and_cursorless() {
        let mut app = test_app();
        app.low_motion = true;
        app.fancy_animations = true;
        app.cursor_position = 7;
        app.ocean_started_at = Instant::now() - Duration::from_secs(2);
        let first = empty_state_lines(&app, Rect::new(0, 0, 100, 30));

        app.ocean_started_at = Instant::now() - Duration::from_secs(11);
        let second = empty_state_lines(&app, Rect::new(0, 0, 100, 30));

        assert_eq!(first, second, "reduced motion must freeze mark and shine");
        assert_eq!(
            app.cursor_position, 7,
            "the empty-state decoration must leave cursor ownership to the composer"
        );
    }

    #[test]
    fn idle_whale_uses_the_human_brand_role_not_focus_blue() {
        let mut app = test_app();
        app.low_motion = true;
        let lines = empty_state_lines(&app, Rect::new(0, 0, 100, 30));
        let colors = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter_map(|span| span.style.fg)
            .collect::<Vec<_>>();

        assert!(colors.contains(&app.ui_theme.accent_action));
        assert_ne!(app.ui_theme.accent_action, app.ui_theme.accent_primary);
    }

    #[test]
    fn idle_whale_caustic_obeys_motion_policy_and_attention_stillness() {
        let mut app = test_app();
        app.launch.visible = false;
        app.low_motion = false;
        app.fancy_animations = true;
        assert!(idle_mark_animation_enabled(&app));

        app.low_motion = true;
        assert!(!idle_mark_animation_enabled(&app));

        app.low_motion = false;
        app.fancy_animations = false;
        assert!(!idle_mark_animation_enabled(&app));

        app.fancy_animations = true;
        app.plan_prompt_pending = true;
        assert!(!idle_mark_animation_enabled(&app));

        app.plan_prompt_pending = false;
        app.ocean_treatment = crate::tui::ocean::OceanTreatment::Classic;
        assert!(!idle_mark_animation_enabled(&app));

        app.ocean_treatment = crate::tui::ocean::OceanTreatment::Ombre;
        app.launch.visible = true;
        assert!(!idle_mark_animation_enabled(&app));

        app.launch.visible = false;
        app.view_stack
            .push(crate::tui::views::HelpView::new_for_locale(app.ui_locale));
        assert!(!idle_mark_animation_enabled(&app));
    }

    #[test]
    fn verifying_phase_meters_a_tick_for_test_runs_only() {
        use crate::tui::active_cell::ActiveCell;
        use crate::tui::history::{ExecCell, ExecSource, HistoryCell, ToolCell, ToolStatus};

        let running_exec = |command: &str| {
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: command.to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: None,
                duration_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            }))
        };

        let mut app = test_app();
        app.runtime_turn_status = Some("in_progress".to_string());
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(3));

        // A live test run reads as `verifying`. Reduced motion keeps the
        // semantic label while sharing the calm, static live-work marker.
        let mut active = ActiveCell::new();
        active.push_tool("exec-1", running_exec("cargo test -p codewhale-tui"));
        app.active_cell = Some(active);
        assert_eq!(ShellPhase::from_app(&app), ShellPhase::Verifying);
        app.low_motion = true;
        let (marker, label) = phase_marker(&app, ShellPhase::Verifying);
        assert_eq!(marker, crate::tui::spinner::BRAILLE_SPINNER_STILL_FRAME);
        assert_eq!(label, "verifying");
        app.low_motion = false;

        // An ordinary build stays `working` — checking must not lie.
        let mut active = ActiveCell::new();
        active.push_tool("exec-2", running_exec("cargo build --release"));
        app.active_cell = Some(active);
        assert_eq!(ShellPhase::from_app(&app), ShellPhase::Working);

        // Verifying is a live phase: strip sits above the composer and
        // shares the live seafoam hue.
        assert!(
            crate::tui::phase_strip::PhaseStripPlacement::for_phase(ShellPhase::Verifying)
                .is_above_composer()
        );
        assert_eq!(
            ShellPhase::Verifying.color(&app),
            app.ui_theme.status_working
        );
    }

    #[test]
    fn attention_and_failure_keep_distinct_semantic_hues() {
        let app = test_app();
        assert_eq!(ShellPhase::Waiting.color(&app), app.ui_theme.accent_action);
        assert_eq!(ShellPhase::Approval.color(&app), app.ui_theme.accent_action);
        assert_eq!(ShellPhase::Failed.color(&app), app.ui_theme.error_fg);
        assert_ne!(
            ShellPhase::Waiting.color(&app),
            ShellPhase::Failed.color(&app)
        );
    }

    #[test]
    fn completion_releases_once_then_settles_to_checkmark() {
        let mut app = test_app();
        app.runtime_turn_status = Some("completed".to_string());
        app.low_motion = false;
        app.fancy_animations = true;
        app.ocean_completion_started_at = Some(Instant::now() - Duration::from_millis(120));

        let (marker, label) = phase_marker(&app, ShellPhase::from_app(&app));
        assert_ne!(marker, "✓");
        assert_eq!(label, "finishing");

        app.ocean_completion_started_at = Some(Instant::now() - Duration::from_millis(700));
        let (marker, label) = phase_marker(&app, ShellPhase::Done);
        assert_eq!(marker, "✓");
        assert_eq!(label, "done");

        app.low_motion = true;
        app.ocean_completion_started_at = Some(Instant::now());
        let (marker, label) = phase_marker(&app, ShellPhase::Done);
        assert_eq!(marker, "✓");
        assert_eq!(label, "done");
    }
}
