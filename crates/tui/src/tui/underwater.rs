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

use crate::localization::{Locale, MessageId, tr};
use crate::tui::{
    app::{App, AppMode, OnboardingState},
    approval::ApprovalMode,
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

const WORKING_BUBBLE_FRAMES: [&str; 8] = ["⠀", "⢀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣿"];
const COMPLETION_BREATH_MS: u128 = 800;
const COMPLETION_RELEASE_MS: u128 = 560;
const IDLE_WHALE_ROWS: [&str; 3] = [
    " ▗▄▄▄▄▄▄▄▄▄▄▄▄▄▖    ▚▞",
    "▐██·████████████▙▄▄▄▞",
    " ▝▀▀▀▀▀▀▀▀▀▀▀▀▀▘",
];
const IDLE_SHIMMER_CYCLE_MS: u128 = 4_000;
const IDLE_SHIMMER_SWEEP_FRACTION: f32 = 0.32;
const IDLE_SHIMMER_BAND_HALF_WIDTH: f32 = 0.38;
const IDLE_SHIMMER_STRENGTH: f32 = 0.33;

impl ShellPhase {
    #[must_use]
    pub fn from_app(app: &App) -> Self {
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
            || app
                .active_cell
                .as_ref()
                .is_some_and(|active| !active.is_empty())
        {
            if verification_run_active(app) {
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

/// True when the live active cell is running a verification-shaped tool:
/// the verifier tool itself or an exec whose program is a known test/check
/// runner. Conservative by design — misclassifying real work as `verifying`
/// would lie; plain `working` never does.
fn verification_run_active(app: &App) -> bool {
    use crate::tui::history::{HistoryCell, ToolCell, ToolStatus};
    let Some(active) = app.active_cell.as_ref() else {
        return false;
    };
    active.entries().iter().any(|cell| {
        let HistoryCell::Tool(tool) = cell else {
            return false;
        };
        match tool {
            ToolCell::Exec(exec) if exec.status == ToolStatus::Running => {
                exec_is_verification(&exec.command)
            }
            ToolCell::Generic(generic) if generic.status == ToolStatus::Running => {
                let name = generic.name.to_ascii_lowercase();
                name.contains("verif") || name == "read_lints"
            }
            _ => false,
        }
    })
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
    if app.low_motion || !app.fancy_animations {
        return None;
    }
    app.ocean_completion_started_at
        .map(|started| started.elapsed().as_millis())
        .filter(|elapsed| *elapsed < COMPLETION_BREATH_MS)
}

pub(crate) fn phase_marker(app: &App, phase: ShellPhase) -> (&'static str, Cow<'static, str>) {
    let locale = app.ui_locale;
    match phase {
        ShellPhase::Idle => ("·", phase.label(locale)),
        ShellPhase::Typing => ("›", phase.label(locale)),
        ShellPhase::Working => {
            // The footer and the live tool card share one wall-clock cadence,
            // so the two primary liveness marks never look like unrelated
            // spinners. The shared helper also preserves the 400ms
            // "motion is earned" delay and reduced/still fallback.
            let frame = crate::tui::spinner::braille_spinner_frame(
                app.turn_started_at,
                app.low_motion || !app.fancy_animations,
            );
            (frame, phase.label(locale))
        }
        ShellPhase::Verifying => {
            // Metered braille tick on the shared live clock — checking, not
            // searching. Reduced motion holds the legible mid frame.
            let frame = crate::tui::spinner::verification_tick_frame(
                app.turn_started_at,
                app.low_motion || !app.fancy_animations,
            );
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
            _ => ("✓", phase.label(locale)),
        },
        ShellPhase::Failed => ("✕", phase.label(locale)),
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

/// Render the one-line shell header. Route, mode, permission, active-agent
/// count, and context each have exactly one owner here.
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
            Style::default().fg(match app.mode {
                AppMode::Plan => app.ui_theme.mode_plan,
                AppMode::Operate => app.ui_theme.mode_operate,
                _ => app.ui_theme.mode_agent,
            }),
        ),
    ];
    if tier != ShellTier::Compact {
        // The Underwater shell owns its header rather than delegating to the
        // classic renderer, so render the selected status mark here too.
        // "cw" is already the leading brand mark; the other choices deserve
        // their visible indicator beside it.
        if let Some(indicator) = crate::tui::widgets::header_status_indicator_frame(
            (!app.low_motion && app.fancy_animations)
                .then_some(app.turn_started_at)
                .flatten(),
            &app.status_indicator,
        )
        .filter(|indicator| *indicator != "cw")
        {
            left.push(Span::raw(" "));
            left.push(Span::styled(
                indicator,
                Style::default()
                    .fg(app.ui_theme.info)
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    // Permission is safety state, not optional chrome. Compact terminals shed
    // the status mark and context meter, but keep the effective posture.
    left.push(Span::styled(
        " · ",
        Style::default().fg(app.ui_theme.text_dim),
    ));
    left.push(Span::styled(
        permission_label(app),
        Style::default().fg(app.ui_theme.text_muted),
    ));

    let mut right = Vec::new();
    if tier != ShellTier::Compact
        && let Some((used, max, percent)) = crate::tui::ui::context_usage_snapshot(app)
    {
        let filled = ((percent / 100.0) * 5.0).ceil().clamp(0.0, 5.0) as usize;
        right.push(Span::styled(
            format!(
                "{}/{} [{}{}] {:.0}%",
                compact_tokens(used),
                compact_tokens(i64::from(max)),
                "▰".repeat(filled),
                "▱".repeat(5usize.saturating_sub(filled)),
                percent
            ),
            Style::default().fg(app.ui_theme.info),
        ));
    }
    if tier == ShellTier::Wide {
        if !right.is_empty() {
            right.push(Span::raw("  "));
        }
        right.push(Span::styled(
            format!("v{}", env!("DEEPSEEK_BUILD_VERSION")),
            Style::default().fg(app.ui_theme.text_hint),
        ));
    }

    let available = usize::from(area.width);
    let right_width = span_width(&right);
    let left_budget = available.saturating_sub(right_width + usize::from(right_width > 0));
    if span_width(&left) > left_budget {
        let mode = mode_label(app.ui_locale, app.mode);
        let permission = permission_label(app);
        let suffix = vec![
            Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
            Span::styled(mode, Style::default().fg(app.ui_theme.accent_primary)),
            Span::styled(" · ", Style::default().fg(app.ui_theme.text_dim)),
            Span::styled(permission, Style::default().fg(app.ui_theme.text_muted)),
        ];
        let fixed_width = 4usize.saturating_add(span_width(&suffix));
        let model_budget = left_budget.saturating_sub(fixed_width);
        left = vec![
            Span::styled(
                "cw",
                Style::default()
                    .fg(app.ui_theme.accent_primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                truncate_to_width(&app.model_display_label(), model_budget),
                Style::default().fg(app.ui_theme.text_muted),
            ),
        ];
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
    crate::tui::motion::MotionPolicy::from_settings(
        app.low_motion,
        app.fancy_animations,
        app.constrained_frame_rate,
    )
    .allows_decorative()
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
        let color = if ch == '·' {
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
        let mut mark = vec![vec![Span::styled(
            "   ˚",
            Style::default().fg(app.ui_theme.accent_secondary),
        )]];
        mark.extend(IDLE_WHALE_ROWS.iter().enumerate().map(|(row, text)| {
            idle_whale_row_spans(
                text,
                row,
                elapsed_ms,
                animated,
                app.ui_theme.accent_primary,
                app.ui_theme.text_body,
                app.ui_theme.text_body,
            )
        }));
        for row in mark {
            let row_width = span_width(&row);
            let inset = " ".repeat(width.saturating_sub(row_width) / 2);
            let mut spans = vec![Span::raw(inset)];
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
        let fleet_label = if tier == ShellTier::Compact {
            tr(app.ui_locale, MessageId::EmptyStateFleetLabel)
        } else {
            tr(app.ui_locale, MessageId::EmptyStateFleetSetupLabel)
        };
        let fleet = format!("{fleet_label}  /fleet setup");
        let inset = " ".repeat(width.saturating_sub(fleet.width()) / 2);
        lines.push(Line::from(Span::styled(
            format!("{inset}{fleet}"),
            Style::default().fg(app.ui_theme.text_hint),
        )));
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
    fn compact_header_keeps_mode_and_effective_permission() {
        let mut app = test_app();
        app.mode = AppMode::Operate;
        app.approval_mode = ApprovalMode::Bypass;
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
        assert_eq!(working, crate::tui::spinner::BRAILLE_SPINNER_FRAMES[7]);
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

        // A live test run reads as `verifying` with the metered tick.
        let mut active = ActiveCell::new();
        active.push_tool("exec-1", running_exec("cargo test -p codewhale-tui"));
        app.active_cell = Some(active);
        assert_eq!(ShellPhase::from_app(&app), ShellPhase::Verifying);
        app.low_motion = true;
        let (marker, label) = phase_marker(&app, ShellPhase::Verifying);
        assert_eq!(marker, crate::tui::spinner::VERIFY_TICK_FRAMES[4]);
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
