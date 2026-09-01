//! Coherent shell grammar for the underwater TUI.
//!
//! This module owns phase, responsive density, the empty-state composition,
//! and the compact header/footer fact budget. Product data still belongs to
//! [`App`]; this is only its terminal projection. Keeping these decisions in
//! one place prevents the default UI from drifting back into a header +
//! sidebar + dashboard + footer composition with four owners for one fact.

use crate::tui::mark::MarkSize;
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
use crate::palette::{ChromeInk, chrome_style};
use crate::tui::{
    app::{App, AppMode, HeaderActionTarget, HeaderHitbox, OnboardingState},
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

/// The launch input model's seven-choice table (main's #5698 authority,
/// kept verbatim): the row a direct key or ↑/↓ focuses, and what Enter
/// dispatches. The Tideline startup stage PROJECTS this table onto its
/// visible rows (`QUICK_ACTION_ROWS`/`OPTION_TILE_ROWS` below); the table
/// itself is no longer rendered anywhere.
const LAUNCH_ROWS: [(MessageId, &str); 7] = [
    (MessageId::LaunchMenuConnect, "P"),
    (MessageId::LaunchMenuResumeSession, "Ctrl+R"),
    (MessageId::LaunchMenuWork, "W"),
    (MessageId::LaunchMenuNewWorktree, "Ctrl+N"),
    (MessageId::LaunchMenuChat, "C"),
    (MessageId::LaunchMenuTheme, "T"),
    (MessageId::LaunchMenuHelp, "F1"),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    None,
    Connect,
    NewSession,
    NewChat,
    CreateWorktree(String),
    Resume,
    Theme,
    Help,
    Changelog,
    Quit,
    /// Submit the composed pre-session message: begin the launch session,
    /// then hand the text to the normal composer dispatch path.
    SendComposer,
}

impl LaunchAction {
    /// Session-only mode selected by a launch choice. The event loop applies
    /// this with `App::set_mode`, never the startup-default-writing selector.
    #[must_use]
    pub const fn session_mode(&self) -> Option<AppMode> {
        match self {
            Self::NewSession => Some(AppMode::Agent),
            Self::NewChat => Some(AppMode::Plan),
            _ => None,
        }
    }
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

    // Tab moves keyboard focus between the startup choices and the
    // pre-session composer — the mouse equivalent is clicking the composer
    // row. The worktree name prompt above keeps its own keys while open.
    if matches!(key.code, KeyCode::Tab)
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
    {
        launch.composer_focus = !launch.composer_focus;
        return LaunchAction::None;
    }

    let direct = match key.code {
        KeyCode::Char('p') | KeyCode::Char('P')
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            Some(0)
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(1),
        KeyCode::Char('w') | KeyCode::Char('W')
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            Some(2)
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(3),
        KeyCode::Char('c') | KeyCode::Char('C')
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            Some(4)
        }
        KeyCode::Char('t') | KeyCode::Char('T')
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER) =>
        {
            Some(5)
        }
        KeyCode::F(1) => Some(6),
        // Changelog and quit remain stable keyboard-only shell actions. They
        // are intentionally outside the seven startup choices in the Tideline
        // contract, so invoking either must not move visible row focus.
        KeyCode::Char('l') | KeyCode::Char('L')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            return LaunchAction::Changelog;
        }
        KeyCode::Char('q') | KeyCode::Char('Q')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            return LaunchAction::Quit;
        }
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
        0 => LaunchAction::Connect,
        1 => LaunchAction::Resume,
        2 => LaunchAction::NewSession,
        3 if launch.worktree_available => {
            launch.worktree_input = Some(String::new());
            launch.status = Some(tr(locale, MessageId::LaunchWorktreePrompt).into_owned());
            // The name prompt owns the keyboard while it is open; the
            // composer must not hold focus underneath it.
            launch.composer_focus = false;
            LaunchAction::None
        }
        3 => {
            launch.status = Some(tr(locale, MessageId::LaunchWorktreeNeedsGit).into_owned());
            LaunchAction::None
        }
        4 => LaunchAction::NewChat,
        5 => LaunchAction::Theme,
        6 => LaunchAction::Help,
        _ => LaunchAction::None,
    }
}

/// What the pre-session composer layer decided about one key while it holds
/// focus.
///
/// This is only an admission guard, never an input implementation: the
/// startup composer is the session's own [`crate::tui::app::ComposerState`],
/// and every editing key is answered by the conversation composer match in
/// the event loop — the single composer input authority — exactly as it
/// would be in a live session. Word motion, selection, completion menus,
/// attachments, history, paste bursts, and vim behaviour therefore cannot
/// drift from the shell. Only three things are launch-specific here: leaving
/// the composer, handing a key to the startup menu, and submitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchComposerKey {
    /// Leave the composer; the key is fully consumed (Esc, Tab, empty
    /// Enter).
    Blur,
    /// Leave the composer and let the same key then drive the menu (plain
    /// Up/Down while no completion menu is open).
    BlurToMenu,
    /// Submit the composed message through the normal dispatch path.
    Submit,
    /// A completion-menu selection was applied (a slash or mention popup was
    /// open and Enter picked the highlighted entry); the key is consumed
    /// without submitting — the completed text stays in the composer.
    MenuSelect,
    /// A chord the startup menu owns (Ctrl+R resume, Ctrl+N worktree,
    /// Ctrl+L changelog, Ctrl+Q quit, F1 help): the same key is then handed
    /// to [`handle_launch_key`]. Startup shortcuts deliberately win over
    /// their composer meanings while the startup screen is up.
    MenuChord,
    /// Not launch-specific: the conversation composer match below owns the
    /// key. The event loop must not run [`handle_launch_key`] for it.
    ComposerAuthority,
}

/// Admit one key for the pre-session composer.
///
/// Editing keys are never handled here — they fall through to the
/// conversation composer match so there is exactly one composer input
/// system. Plain startup shortcut letters (p/w/c/t) intentionally lose to
/// typing while the composer holds focus; their chords (Ctrl+R/N/L/Q, F1)
/// stay menu-owned via [`LaunchComposerKey::MenuChord`].
pub fn handle_launch_composer_key(app: &mut App, key: KeyEvent) -> LaunchComposerKey {
    let multiline = app.composer_multiline_mode;
    match key.code {
        KeyCode::Esc | KeyCode::Tab => {
            app.launch.composer_focus = false;
            LaunchComposerKey::Blur
        }
        KeyCode::Up | KeyCode::Down
            // Completion menus stay composer-owned: navigating their entries
            // must match the conversation composer exactly.
            if crate::tui::slash_menu::visible_slash_menu_entries(app, 1).is_empty()
                && crate::tui::file_mention::visible_mention_menu_entries(app, 1).is_empty() =>
        {
            app.launch.composer_focus = false;
            LaunchComposerKey::BlurToMenu
        }
        KeyCode::Enter
            if crate::tui::composer_ui::composer_submit_chord(key, multiline).is_some() =>
        {
            // #573 parity with the session composer's Enter arm: when a
            // completion popup is matching (e.g. `/mo` → `/model`), Enter
            // applies the highlighted entry instead of sending the literal
            // prefix. A mention completion amends the composed text and is
            // consumed; a slash completion completes the command and falls
            // through to Submit so the launch dispatch path executes it.
            let mention_entries =
                crate::tui::file_mention::visible_mention_menu_entries(app, 1);
            if !mention_entries.is_empty()
                && crate::tui::file_mention::apply_mention_menu_selection(
                    app,
                    &mention_entries,
                )
            {
                return LaunchComposerKey::MenuSelect;
            }
            let slash_entries = crate::tui::slash_menu::visible_slash_menu_entries(app, 1);
            if !slash_entries.is_empty() {
                crate::tui::slash_menu::apply_slash_menu_selection(app, &slash_entries, false);
                app.close_slash_menu();
            }
            if app.input.trim().is_empty() {
                app.launch.composer_focus = false;
                LaunchComposerKey::Blur
            } else {
                LaunchComposerKey::Submit
            }
        }
        KeyCode::Char('r' | 'n' | 'l' | 'q')
            if key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            LaunchComposerKey::MenuChord
        }
        KeyCode::F(1) => LaunchComposerKey::MenuChord,
        // Every other key — text, caret motion, word motion, selection,
        // newline chords, Home/End, kill/chord editing, vim motions — is
        // answered by the conversation composer authority.
        _ => LaunchComposerKey::ComposerAuthority,
    }
}

impl ShellTier {
    // `for_area` (the two-dimensional variant) went with the empty state's
    // tier branch: the idle caption sheds detail continuously now, so nothing
    // was left that wanted a coarse three-way answer about a whole Rect. The
    // row and column floors it encoded still exist, spelled out as
    // `AMBIENT_MIN_CHAT_HEIGHT` / `AMBIENT_MIN_CHAT_WIDTH` where the layout
    // can honour them.
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
    Compacting,
    AutoCompacting,
    Reasoning,
    Reading,
    UsingTool,
    UsingSubagents,
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
        let kind = if app
            .active_compaction
            .as_ref()
            .is_some_and(|compaction| compaction.auto)
        {
            LiveActivityKind::AutoCompacting
        } else if app.active_compaction.is_some() {
            LiveActivityKind::Compacting
        } else if tools.verifying {
            LiveActivityKind::Verifying
        } else if app_has_unfinished_subagents(app) {
            LiveActivityKind::UsingSubagents
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
            LiveActivityKind::Compacting => tr(locale, MessageId::ContextManualCompacting),
            LiveActivityKind::AutoCompacting => tr(locale, MessageId::ContextAutoCompacting),
            LiveActivityKind::Reasoning => tr(locale, MessageId::PhaseReasoning),
            LiveActivityKind::Reading => tr(locale, MessageId::PhaseReading),
            LiveActivityKind::UsingTool => tr(locale, MessageId::PhaseUsingTool),
            LiveActivityKind::UsingSubagents => tr(locale, MessageId::PhaseSubagents),
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

/// True when any sub-agent spawned by this session is still running: live
/// progress rows win over the cache, whose Running entries are the persisted
/// view of the same actors.
fn app_has_unfinished_subagents(app: &App) -> bool {
    !app.agent_progress.is_empty()
        || app.subagent_cache.iter().any(|agent| {
            matches!(
                agent.status,
                crate::tools::subagent::SubAgentStatus::Running
            )
        })
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
use super::ocean::COMPLETION_BREATH_MS;
const COMPLETION_RELEASE_MS: u128 = 560;
// The idle whale portrait rows (IDLE_WHALE_ROWS / UWU_IDLE_WHALE_ROWS) and
// their caustic shimmer were deleted per the 2026-08-29 founder directive:
// hand-drawn whale art is out; the only sanctioned terminal mark is the one
// generated from the brand master path. The ambient empty-state surface
// (wordmark, context caption, prompt) below is not whale art and stays.

impl ShellPhase {
    #[must_use]
    pub fn from_app(app: &App) -> Self {
        Self::from_app_with_activity(app, LiveActivity::from_app(app))
    }

    #[must_use]
    pub(crate) fn from_app_with_activity(app: &App, activity: LiveActivity) -> Self {
        if matches!(
            app.view_stack.top_kind(),
            Some(ModalKind::Approval | ModalKind::Elevation | ModalKind::UserInput)
        ) {
            return Self::Approval;
        }
        if matches!(
            activity.kind(),
            LiveActivityKind::Compacting | LiveActivityKind::AutoCompacting
        ) {
            // A typed CompactionStarted event is newer and more specific than
            // a prior turn's failed projection. Keep the recovery operation
            // visible until its matching terminal event arrives.
            return Self::Working;
        }
        if app.turn_error_posted
            || matches!(app.runtime_turn_status.as_deref(), Some("failed" | "error"))
        {
            return Self::Failed;
        }
        if app.pending_user_input_prompt.is_some()
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
        if !app.input.is_empty() {
            return Self::Typing;
        }
        if matches!(app.runtime_turn_status.as_deref(), Some("completed")) {
            return Self::Done;
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
    #[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
    // (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
    pub fn color(self, app: &App) -> Color {
        phase_ink(self).color(&app.ui_theme)
    }
}

/// Status-bar phase ink. Failure red is only `Failed`.
#[must_use]
pub(crate) fn phase_ink(phase: ShellPhase) -> ChromeInk {
    match phase {
        ShellPhase::Idle => ChromeInk::Metadata,
        ShellPhase::Done => ChromeInk::Outcome,
        ShellPhase::Typing => ChromeInk::Identity,
        // Verifying shares the live seafoam hue; the tick-vs-bubble
        // marker carries the checking/searching distinction.
        ShellPhase::Working | ShellPhase::Verifying => ChromeInk::Active,
        ShellPhase::Waiting | ShellPhase::Approval => ChromeInk::Waiting,
        ShellPhase::Failed => ChromeInk::Failure,
    }
}

/// Exhaustive on purpose: a new [`AppMode`] must be handed a Policy ink
/// deliberately rather than inheriting act's by falling through a wildcard.
fn header_mode_ink(mode: AppMode) -> ChromeInk {
    match mode {
        AppMode::Plan => ChromeInk::PolicyPlan,
        AppMode::Operate => ChromeInk::PolicyOperate,
        // YOLO stays Policy, not Failure — the header must not spend red
        // on a selected mode. It wears the act badge because `mode_label`
        // resolves it to act; the posture it implies is the permission
        // chip's Cognition ink, not this one.
        AppMode::Agent | AppMode::Auto | AppMode::Yolo => ChromeInk::PolicyAct,
    }
}

fn header_permission_ink(mode: ApprovalMode) -> ChromeInk {
    match mode {
        ApprovalMode::Suggest | ApprovalMode::Never => ChromeInk::PermissionAsk,
        ApprovalMode::Auto => ChromeInk::PermissionAutoReview,
        ApprovalMode::Bypass => ChromeInk::PermissionFullAccess,
    }
}

#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
fn header_fg(app: &App, ink: ChromeInk) -> Style {
    chrome_style(&app.ui_theme, ink)
}

/// One posture word with its ink — the unit the classic header's lockup was
/// made of, now carried as merged-footer chips.
pub(crate) type PostureChip = (Cow<'static, str>, ChromeInk);

/// The posture lockup as two standalone chips for the Tideline merged
/// footer (spec §3: the old header's mode/permission chips move into the
/// footer activity segment). Same words, same inks, and the same mapping
/// the classic header used — [`header_mode_ink`] for the mode word,
/// [`header_permission_ink`] for the permission phrase. The filesystem
/// scope notice, when it deviates, folds into the permission chip's text
/// (the header already painted it in the permission ink).
pub(crate) fn posture_chips(app: &App) -> (Option<PostureChip>, Option<PostureChip>) {
    let mode = (
        mode_label(app.ui_locale, app.mode),
        header_mode_ink(app.mode),
    );
    let mut permission = (
        permission_label(app),
        header_permission_ink(app.approval_mode),
    );
    if let Some(scope) = filesystem_scope_notice(app) {
        permission.0 = format!("{} · {scope}", permission.0).into();
    }
    (Some(mode), Some(permission))
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

/// Truthful window-title activity verb for the OSC-0 whale animation.
///
/// Uses short English fragments (with fixed-width ellipsis) so alt-tabbed
/// sessions stay legible without depending on the full localized phase strip.
#[must_use]
pub(crate) fn title_activity_verb(app: &App) -> &'static str {
    let activity = LiveActivity::from_app(app);
    let phase = ShellPhase::from_app_with_activity(app, activity);
    match phase {
        ShellPhase::Waiting | ShellPhase::Approval => "waiting on you…",
        ShellPhase::Verifying => "verifying…",
        ShellPhase::Done => "done",
        ShellPhase::Failed => "failed",
        ShellPhase::Typing => "drafting…",
        ShellPhase::Idle => "idle",
        ShellPhase::Working => match activity.kind() {
            LiveActivityKind::Compacting | LiveActivityKind::AutoCompacting => {
                "compacting context…"
            }
            LiveActivityKind::Reasoning => "reasoning…",
            LiveActivityKind::Reading => "reading…",
            LiveActivityKind::UsingTool => "using tool…",
            LiveActivityKind::UsingSubagents => "pod underway…",
            LiveActivityKind::Verifying => "verifying…",
            LiveActivityKind::Working => "in the current…",
        },
    }
}

/// Push the current shell phase into the terminal title whale animation.
pub(crate) fn sync_title_activity(app: &App) {
    crate::tui::notifications::set_title_motion_enabled(
        app.motion_policy().allows_decorative() && app.status_indicator != "off",
    );
    // Keep the `[title] …` window-title prefix in step with the session and
    // config defaults; change detection inside makes this free when nothing
    // moved.
    crate::tui::notifications::set_title_prefix(app.window_title_prefix());
    if app.is_loading
        || matches!(
            ShellPhase::from_app(app),
            ShellPhase::Working
                | ShellPhase::Verifying
                | ShellPhase::Waiting
                | ShellPhase::Approval
                | ShellPhase::Typing
        )
    {
        crate::tui::notifications::set_title_activity_verb(title_activity_verb(app));
    }
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
///
/// Tool-approval posture only. Filesystem scope is a separate fact and only
/// earns header columns when it is worth reading — see
/// [`filesystem_scope_notice`].
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
        // constitution rules. This is **tool-approval posture**, not
        // filesystem scope — see filesystem_scope_notice.
        ApprovalMode::Bypass => tr(locale, MessageId::ChipPermissionFullAccess),
        ApprovalMode::Never => tr(locale, MessageId::ChipPermissionNever),
    }
}

/// The effective filesystem scope — but only when it says something the
/// permission word beside it does not already say.
///
/// This chip exists because "Full Access" (tool approval) was being read as
/// unrestricted disk writes (user report, 2026-07-23), and because a policy
/// with no enforcement backend used to name a boundary nobody applied
/// (2026-08-04 audit). Both of those are deviations. The default — an
/// enforced workspace-write boundary — is what every ordinary session already
/// has, and printing `files: workspace` on every frame of every session spent
/// seventeen columns of the primary chrome saying so. A notice that is always
/// on cannot signal anything; folding the expected case away is what lets
/// `files: full disk` and `files: workspace (unenforced)` land as warnings
/// when they do appear.
///
/// `read-only` under Plan is dropped for the same reason from the other side:
/// the permission word there is already the literal phrase "read only".
#[must_use]
fn filesystem_scope_notice(app: &App) -> Option<Cow<'static, str>> {
    // Spelled out because the old `fs:` prefix read as an unexplained
    // acronym (user report, 2026-07-23): this chip states which files the
    // session may write.
    let policy = crate::core::authority::sandbox_policy_for_turn(
        app.mode,
        app.approval_mode,
        app.configured_sandbox_mode.as_deref(),
        &app.workspace,
        crate::core::authority::SandboxNetworkAccess::from_config(app.configured_sandbox_network),
    );
    // A policy is an intent; enforcement needs a backend. On default Linux
    // (bubblewrap is opt-in) and on all Windows there is none. Say
    // "unenforced" rather than name a boundary that is not applied.
    // `DangerFullAccess` is already honest, and `ExternalSandbox` is enforced
    // by the external runner, not by us.
    let unenforced = app.sandbox_backend.is_none()
        && !matches!(
            policy,
            crate::sandbox::SandboxPolicy::DangerFullAccess
                | crate::sandbox::SandboxPolicy::ExternalSandbox { .. }
        );
    match policy {
        crate::sandbox::SandboxPolicy::ReadOnly if unenforced => {
            Some(Cow::Borrowed("files: read-only (unenforced)"))
        }
        crate::sandbox::SandboxPolicy::ReadOnly => {
            (app.mode != AppMode::Plan).then_some(Cow::Borrowed("files: read-only"))
        }
        crate::sandbox::SandboxPolicy::DangerFullAccess => Some(Cow::Borrowed("files: full disk")),
        crate::sandbox::SandboxPolicy::ExternalSandbox { .. } => {
            Some(Cow::Borrowed("files: external sandbox"))
        }
        crate::sandbox::SandboxPolicy::WorkspaceWrite { .. } if unenforced => {
            Some(Cow::Borrowed("files: workspace (unenforced)"))
        }
        // The unremarkable case: writes are confined to the workspace and the
        // OS is actually enforcing it. Saying so on every frame of every
        // session spends the header on a fact nobody is asking about — with
        // one exception. When the permission chip reads "Full Access", the
        // scope chip is the only thing on screen that says the writes are
        // still confined. Suppressing it there recreates precisely the
        // misreading the chip was added for (tool-approval "Full Access" taken
        // to mean unrestricted disk writes), and that pairing is reachable:
        // Bypass with a configured `workspace-write` is clamped to this policy
        // by `sandbox_policy_for_turn`.
        crate::sandbox::SandboxPolicy::WorkspaceWrite { .. } => {
            (app.approval_mode == ApprovalMode::Bypass).then_some(Cow::Borrowed("files: workspace"))
        }
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

fn render_launch_content_line(
    area: Rect,
    buf: &mut Buffer,
    y: u16,
    inset: u16,
    spans: Vec<Span<'static>>,
) {
    if y >= area.height {
        return;
    }
    let inset = inset.min(area.width / 2);
    Paragraph::new(Line::from(spans)).render(
        Rect {
            x: area.x.saturating_add(inset),
            y: area.y.saturating_add(y),
            width: area.width.saturating_sub(inset.saturating_mul(2)),
            height: 1,
        },
        buf,
    );
}

/// Where the pre-session composer strip docks inside the startup stage.
///
/// The dock owns the stage spacer's bottom rows (spec §5b: composer
/// `Length(4)` incl. border, below the option strip and above the merged
/// footer). At its full size, the shared rounded shell uses the two interior
/// rows for input and localized submit guidance. Compact terminals retain the
/// one-line projection rather than claiming borders they cannot render.
/// Rows are stage-relative; `None` when the stage cannot fit even the input
/// row.
fn launch_composer_rows(stage: Rect) -> Option<(u16, u16)> {
    let dock = startup_layout(stage).dock;
    let input_y = if dock.height >= crate::tui::composer_chrome::TIDELINE_COMPOSER_HEIGHT {
        dock.y.saturating_sub(stage.y).saturating_add(1)
    } else {
        dock.y.saturating_sub(stage.y)
    };
    (dock.height >= 1).then_some((input_y, input_y.saturating_add(1)))
}

fn launch_compact_composer_rows(stage: Rect) -> Option<(u16, u16)> {
    let dock = startup_layout(stage).dock;
    let input_y = dock.y.saturating_sub(stage.y);
    (dock.height >= 1).then_some((input_y, input_y.saturating_add(1)))
}

/// The line the caret sits on in a multi-line composer buffer, plus the
/// caret's column within that line. The launch strip projects one row, so a
/// Shift+Enter newline is truthfully shown as the line being edited.
fn launch_cursor_line(text: &str, caret: usize) -> (&str, usize) {
    let mut consumed = 0usize;
    for line in text.split('\n') {
        let len = line.chars().count();
        if caret <= consumed + len {
            return (line, caret - consumed);
        }
        consumed += len + 1;
    }
    ("", 0)
}

/// Visible `(before_caret, after_caret)` text for the caret's line so the
/// single-row projection keeps the caret on screen while editing.
fn launch_caret_window(line: &str, caret_col: usize, budget: usize) -> (String, String) {
    let chars: Vec<char> = line.chars().collect();
    let caret_col = caret_col.min(chars.len());
    // The budget is display columns, not characters: CJK and emoji occupy
    // two cells, and a character-count slice let a wide draft push the caret
    // past the clip end (review finding 4 — the caret vanished on
    // CJK/emoji-heavy lines because the downstream truncation cuts from the
    // end). Accumulate backward from the caret by rendered width so the
    // caret always lands inside the budget, then fill forward with whatever
    // width remains.
    let before_budget = budget.saturating_sub(1);
    let mut before = String::new();
    let mut before_width = 0usize;
    for &ch in chars[..caret_col].iter().rev() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if before_width.saturating_add(ch_width) > before_budget {
            break;
        }
        before_width += ch_width;
        before.insert(0, ch);
    }
    let after_budget = budget.saturating_sub(before_width + 1);
    let mut after = String::new();
    let mut after_width = 0usize;
    for &ch in chars[caret_col..].iter() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if after_width.saturating_add(ch_width) > after_budget {
            break;
        }
        after_width += ch_width;
        after.push(ch);
    }
    (before, after)
}

/// Same caret convention as the worktree name prompt on this screen: a
/// static block that low_motion renders as an underscore.
fn launch_cursor_glyph(low_motion: bool) -> &'static str {
    if low_motion { "_" } else { "▌" }
}

/// Paint the active completion popup for the launch composer (#5698 review
/// finding 2: the menus existed — the conversation composer match drove
/// them — but the launch screen returned before `ComposerWidget`, so typing
/// `/mo` showed nothing). A compact list directly above the input row; the
/// same entries, the same selected-row convention, and the same mention-
/// before-slash precedence as the session popup inside `ComposerWidget`.
pub fn render_launch_completion_popup(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    input_y: u16,
    slash_menu_entries: &[crate::tui::widgets::SlashMenuEntry],
    mention_menu_entries: &[String],
) {
    if !app.launch.composer_focus {
        return;
    }
    // Rows are (marker, label, description) rendered as one inset line.
    let rows: Vec<(bool, String, String)> = if !mention_menu_entries.is_empty() {
        let selected = app
            .mention_menu_selected
            .min(mention_menu_entries.len().saturating_sub(1));
        mention_menu_entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (i == selected, format!("@{entry}"), String::new()))
            .collect()
    } else if !slash_menu_entries.is_empty() {
        let selected = app
            .slash_menu_selected
            .min(slash_menu_entries.len().saturating_sub(1));
        slash_menu_entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let label = if let Some(ref hint) = e.alias_hint {
                    format!("{} or /{}", e.name, hint)
                } else {
                    e.name.clone()
                };
                (i == selected, label, e.description.clone())
            })
            .collect()
    } else {
        return;
    };

    // Popup rows stack upward from the composer input row; never past the
    // header rule, and never more than eight.
    let max_rows = (input_y.saturating_sub(2) as usize).min(8);
    if max_rows == 0 {
        return;
    }
    // Show the tail around the selection like the session popup scrolls.
    let total = rows.len();
    let selected_idx = rows.iter().position(|(sel, _, _)| *sel).unwrap_or(0);
    let top = if total <= max_rows {
        0
    } else {
        let half = max_rows / 2;
        if selected_idx <= half {
            0
        } else if selected_idx + half >= total {
            total - max_rows
        } else {
            selected_idx - half
        }
    };
    for (offset, (is_selected, label, description)) in rows
        .iter()
        .enumerate()
        .skip(top)
        .take(max_rows)
        .map(|(_, row)| row.clone())
        .enumerate()
    {
        let y = input_y - 1 - offset as u16;
        let style = if is_selected {
            crate::tui::menu_style::selected_row_bg_style().fg(crate::palette::SELECTION_TEXT)
        } else {
            Style::default().fg(app.ui_theme.text_muted)
        };
        let marker = crate::tui::glyphs::selection_marker(is_selected);
        let mut line = format!("{marker} {label}");
        if !description.is_empty() {
            let used = line.width();
            let budget = usize::from(area.width)
                .saturating_sub(4)
                .saturating_sub(used + 2);
            if budget > 1 {
                line.push_str("  ");
                line.push_str(&truncate_to_width(description.as_str(), budget));
            }
        }
        render_launch_content_line(
            area,
            buf,
            y,
            2,
            vec![Span::styled(
                truncate_to_width(&line, usize::from(area.width).saturating_sub(4)),
                style,
            )],
        );
    }
}

/// The pre-session composer's display projection — everything the docked
/// strip paints, injected so the startup stage stays a deterministic
/// widget for golden buffers (the everything-injectable law
/// `TidelineStartup` follows). Built from `App` by
/// [`LaunchComposerDisplay::from_app`]; the row painting itself is
/// `render_launch_composer` — #5698's docked strip, reused line-for-line
/// and re-docked below the option strip.
#[derive(Debug, Clone)]
pub struct LaunchComposerDisplay<'a> {
    /// Whether the composer holds keyboard focus (`launch.composer_focus`).
    pub focused: bool,
    /// The session composer's own draft (`composer_display_input`).
    pub input: &'a str,
    /// The caret position inside `input` (`composer_display_cursor`).
    pub caret: usize,
    /// Low-motion mode renders the caret as an underscore.
    pub low_motion: bool,
    /// The blurred, empty composer's placeholder.
    pub placeholder: std::borrow::Cow<'a, str>,
    /// The focused composer's hint line.
    pub hint_focused: std::borrow::Cow<'a, str>,
    /// The blurred composer's hint line.
    pub hint_blurred: std::borrow::Cow<'a, str>,
    /// Mirrors the shared composer preference. The default Tideline startup
    /// surface uses the rounded enclosure; an explicit compact opt-out keeps
    /// the legacy one-line projection only where the setting asks for it.
    pub enclosed: bool,
}

impl Default for LaunchComposerDisplay<'_> {
    fn default() -> Self {
        Self {
            focused: false,
            input: "",
            caret: 0,
            low_motion: false,
            placeholder: Cow::Borrowed(""),
            hint_focused: Cow::Borrowed(""),
            hint_blurred: Cow::Borrowed(""),
            enclosed: true,
        }
    }
}

impl<'a> LaunchComposerDisplay<'a> {
    /// Project the session's own composer state — the single input
    /// authority; the launch dock only re-frames it.
    #[must_use]
    pub fn from_app(app: &'a App) -> Self {
        Self {
            focused: app.launch.composer_focus,
            input: app.composer_display_input(),
            caret: app.composer_display_cursor(),
            low_motion: app.low_motion,
            placeholder: tr(app.ui_locale, MessageId::ComposerPlaceholder),
            hint_focused: tr(app.ui_locale, MessageId::LaunchComposerHint),
            hint_blurred: tr(app.ui_locale, MessageId::LaunchComposerFocusHint),
            enclosed: app.composer_border,
        }
    }
}

/// Draw the docked pre-session composer strip: the session's own
/// `ComposerState` projected as one bottom-docked row — prompt glyph,
/// caret line or placeholder, and a send glyph — with its hint line
/// beneath. This is the same composer state the conversation view edits,
/// not a second input system; only the geometry is the startup stage's
/// dock.
fn render_launch_composer(
    area: Rect,
    buf: &mut Buffer,
    theme: &UiTheme,
    display: &LaunchComposerDisplay<'_>,
    input_y: u16,
    hint_y: u16,
    panel_area: Option<Rect>,
    status_line: Option<&str>,
    ascii_safe: bool,
) {
    let focused = display.focused;
    if let Some(panel_area) = panel_area {
        crate::tui::composer_chrome::render_tideline_composer_shell(
            panel_area, buf, theme, focused, ascii_safe,
        );
        let geometry = crate::tui::composer_chrome::tideline_composer_geometry(panel_area);
        let content_width = usize::from(geometry.content.width);
        if content_width == 0 {
            return;
        }
        let text_budget = content_width.saturating_sub(2);
        let prompt_style = if focused {
            theme.accent_primary
        } else {
            theme.text_hint
        };
        let input = display.input;
        let caret = launch_cursor_glyph(display.low_motion);
        let body = if input.is_empty() {
            if focused {
                caret.to_string()
            } else {
                display.placeholder.to_string()
            }
        } else if focused {
            let (line, col) = launch_cursor_line(input, display.caret);
            let (before, after) = launch_caret_window(line, col, text_budget);
            format!("{before}{caret}{after}")
        } else {
            let (line, _) = launch_cursor_line(input, display.caret);
            line.to_string()
        };
        let body_style = if focused {
            theme.text_body
        } else if input.is_empty() {
            theme.text_hint
        } else {
            theme.text_muted
        };
        Paragraph::new(Line::from(vec![
            Span::styled("❯", Style::default().fg(prompt_style)),
            Span::raw(" "),
            Span::styled(
                truncate_to_width(&body, text_budget),
                Style::default().fg(body_style),
            ),
        ]))
        .render(
            Rect {
                x: geometry.content.x,
                y: geometry.content.y,
                width: content_width as u16,
                height: 1,
            },
            buf,
        );

        let hint = status_line.map(Cow::Borrowed).unwrap_or_else(|| {
            if focused {
                display.hint_focused.clone()
            } else {
                display.hint_blurred.clone()
            }
        });
        if geometry.content.height >= 2 {
            Paragraph::new(Line::from(Span::styled(
                truncate_to_width(hint.as_ref(), content_width),
                Style::default().fg(if status_line.is_some() {
                    theme.text_body
                } else if focused {
                    theme.text_hint
                } else {
                    theme.text_dim
                }),
            )))
            .render(
                Rect {
                    x: geometry.content.x,
                    y: geometry.content.y.saturating_add(1),
                    width: content_width as u16,
                    height: 1,
                },
                buf,
            );
        }
        return;
    }

    let content_width = usize::from(area.width).saturating_sub(4);
    if content_width == 0 {
        return;
    }
    // Inside the row: prompt glyph + space up front, the send affordance's
    // last two columns, and the input between them.
    let text_budget = content_width.saturating_sub(4);
    let prompt_style = if focused {
        theme.accent_primary
    } else {
        theme.text_hint
    };
    let mut spans = vec![
        Span::styled("❯", Style::default().fg(prompt_style)),
        Span::raw(" "),
    ];

    let input = display.input;
    let caret = launch_cursor_glyph(display.low_motion);
    let body = if input.is_empty() {
        if focused {
            caret.to_string()
        } else {
            display.placeholder.to_string()
        }
    } else if focused {
        let (line, col) = launch_cursor_line(input, display.caret);
        let (before, after) = launch_caret_window(line, col, text_budget);
        format!("{before}{caret}{after}")
    } else {
        let (line, _) = launch_cursor_line(input, display.caret);
        line.to_string()
    };
    let body_style = if focused {
        theme.text_body
    } else if input.is_empty() {
        theme.text_hint
    } else {
        theme.text_muted
    };
    let body = truncate_to_width(&body, text_budget);
    let body_width = body.width();
    spans.push(Span::styled(body, Style::default().fg(body_style)));

    let send_style = if input.trim().is_empty() {
        theme.text_hint
    } else {
        theme.accent_action
    };
    spans.push(Span::raw(
        " ".repeat(text_budget.saturating_sub(body_width)),
    ));
    spans.push(Span::styled(" ↑", Style::default().fg(send_style)));
    render_launch_content_line(area, buf, input_y, 2, spans);

    // In the dock's compact tiers the hint row is the shared prompt row —
    // the stage's transient status line paints over it after — so the row
    // only has to exist inside the stage.
    if hint_y < area.height {
        let hint = if focused {
            display.hint_focused.clone()
        } else {
            display.hint_blurred.clone()
        };
        render_launch_content_line(
            area,
            buf,
            hint_y,
            2,
            vec![Span::styled(
                truncate_to_width(hint.as_ref(), content_width),
                Style::default().fg(if focused {
                    theme.text_hint
                } else {
                    theme.text_dim
                }),
            )],
        );
    }
}
#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
fn compact_tokens(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.0}K", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

#[allow(dead_code)]
// classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
/// The context meter is one measured fact: an exact percentage for scanning,
/// a token fraction for auditability when room permits, and a short bar for
/// peripheral vision. It is deliberately the final header fact so its rect
/// stays stable and can point at the inspector without parsing rendered text.
fn header_context_meter(app: &App, tier: ShellTier) -> Option<Span<'static>> {
    crate::tui::ui::context_usage_snapshot(app).map(|(used, max, percent)| {
        let filled = ((percent / 100.0) * 5.0).ceil().clamp(0.0, 5.0) as usize;
        let percentage = format!("{percent:.0}%");
        let text = match tier {
            ShellTier::Compact => format!("ctx {percentage}"),
            ShellTier::Normal | ShellTier::Wide => format!(
                "context {percentage} {}/{} {}{}",
                compact_tokens(used),
                compact_tokens(i64::from(max)),
                "▰".repeat(filled),
                "▱".repeat(5usize.saturating_sub(filled)),
            ),
        };
        Span::styled(text, header_fg(app, ChromeInk::Info))
    })
}

/// Return concrete, typed header targets for the latest frame.
///
/// The context meter is right-aligned and always the final header span, so
/// its visible geometry does not depend on optional git/token facts. The
/// keyboard route remains `Alt+C`; this gives that same inspectable fact a
/// mouse route without inventing another context screen or state owner.
#[allow(dead_code)]
// classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
// Its posture-floor guard (a hitbox never claims overlapped cells) is the
// discipline `topbar::context_meter_hitbox` carries forward.
#[must_use]
pub(crate) fn header_hitboxes(area: Rect, app: &App) -> Vec<HeaderHitbox> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let tier = ShellTier::for_chrome_width(area.width);
    let Some(meter) = header_context_meter(app, tier) else {
        return Vec::new();
    };
    let width = u16::try_from(span_width(&[meter]))
        .unwrap_or(area.width)
        .min(area.width);
    if width == 0 {
        return Vec::new();
    }
    // The posture lockup is the header's guaranteed floor and is never
    // truncated to make room for the right cluster (see
    // render_header_with_git_status). At compact widths that floor can run
    // into the meter's columns, so a hitbox anchored blindly at the right
    // edge would claim cells the posture actually paints (review finding 5).
    // Recompute the floor's width with the same spans the renderer composes
    // and refuse the hitbox when the two would overlap.
    let mut posture_width = 0usize;
    if let Some(indicator) = crate::tui::widgets::header_status_indicator_frame(
        (!app.low_motion && app.fancy_animations)
            .then_some(app.turn_started_at)
            .flatten(),
        &app.status_indicator,
    ) {
        posture_width += indicator.width() + GROUP_GAP.len();
    }
    posture_width += mode_label(app.ui_locale, app.mode).width();
    posture_width += FIELD_JOIN.len() + permission_label(app).width();
    if let Some(scope) = filesystem_scope_notice(app) {
        posture_width += FIELD_JOIN.len() + scope.width();
    }
    let meter_start = usize::from(area.width.saturating_sub(width));
    if meter_start <= posture_width.saturating_add(usize::from(width > 0)) {
        return Vec::new();
    }
    vec![HeaderHitbox {
        area: Rect {
            x: area.x.saturating_add(area.width.saturating_sub(width)),
            y: area.y,
            width,
            height: 1,
        },
        target: HeaderActionTarget::InspectContext,
    }]
}

fn session_token_breakdown(app: &App) -> Option<Span<'static>> {
    app.header_items.contains(&HeaderItem::Tokens).then(|| {
        Span::styled(
            format!(
                "{} in · {} cch · {} out",
                format_token_count_compact(u64::from(app.session.displayed_total_input_tokens())),
                format_token_count_compact(u64::from(
                    app.session.displayed_total_cache_hit_tokens(),
                )),
                format_token_count_compact(u64::from(app.session.displayed_total_output_tokens())),
            ),
            header_fg(app, ChromeInk::Info),
        )
    })
}

/// The header speaks with exactly two separators, and each one means one
/// thing.
///
/// [`FIELD_JOIN`] binds words that qualify one another into a single phrase:
/// `work · ask` is one statement of posture, not two facts. [`GROUP_GAP`]
/// stands between whole facts — posture, then the goal chip, then the update
/// notice; workspace, then the context meter.
///
/// Before this, every one of those boundaries was the same dotted separator at
/// the same dim ink, so the header read as an undifferentiated list and there
/// was nothing for the eye to group on. The gap is deliberately wider than the
/// visual whitespace inside `" · "` — four blank columns against one — because
/// that ratio is the only thing carrying the grouping.
#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
const FIELD_JOIN: &str = " · ";
#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
const GROUP_GAP: &str = "    ";

/// Append one chrome element, inserting the group separator only between
/// elements so an absent element never leaves trailing padding.
#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
fn push_chrome(spans: &mut Vec<Span<'static>>, span: Span<'static>) {
    if !spans.is_empty() {
        spans.push(Span::raw(GROUP_GAP));
    }
    spans.push(span);
}

/// Render the one-line shell header. Immediate operating posture and workspace
/// truth live here; quieter route identity lives beside the phase footer.
#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
pub fn render_header(area: Rect, buf: &mut Buffer, app: &App) {
    let git_status = crate::tui::git_status::cached_status();
    render_header_with_git_status(area, buf, app, &git_status);
}

#[allow(dead_code)] // classic header/band renderer: superseded by the Tideline shell
// (topbar + merged footer, spec §3, 2026-08-29); deletion is its own slice.
fn render_header_with_git_status(
    area: Rect,
    buf: &mut Buffer,
    app: &App,
    git_status: &crate::tui::git_status::GitStatusSnapshot,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let tier = ShellTier::for_chrome_width(area.width);
    Block::default()
        .style(Style::default().bg(app.ui_theme.header_bg))
        .render(area, buf);

    let mode_color = header_mode_ink(app.mode).color(&app.ui_theme);
    // Match the composer's warm top edge exactly: Ask amber, Auto-Review
    // Signal Gold, and Full Access coral.
    let permission_color = header_permission_ink(app.approval_mode).color(&app.ui_theme);
    let dim = header_fg(app, ChromeInk::MetadataDim);
    // `status_indicator` owns the single header mark. It used to be filtered
    // against the literal "cw" because the header also hardcoded a leading
    // "cw" span, and `header_status_indicator_frame` collapses `cw`, the
    // legacy `whale` opt-in, and unknown values onto that same mark — so the
    // filter silently discarded three of the setting's four documented values
    // and left `off` with nothing to turn off (#5512). There is one mark now,
    // and this setting decides what occupies it.
    let status_indicator = crate::tui::widgets::header_status_indicator_frame(
        (!app.low_motion && app.fancy_animations)
            .then_some(app.turn_started_at)
            .flatten(),
        &app.status_indicator,
    );
    // The posture lockup: mark, then mode and permission (and the filesystem
    // scope when it deviates) joined into one phrase. This is the guaranteed
    // floor of the header — everything after it is sheddable — so it is built
    // once and reused by the cramped rebuild below rather than spelled twice.
    let mut left = Vec::new();
    if let Some(indicator) = status_indicator {
        left.push(Span::styled(
            indicator,
            header_fg(app, ChromeInk::Identity).add_modifier(Modifier::BOLD),
        ));
        left.push(Span::raw(GROUP_GAP));
    }
    left.push(Span::styled(
        mode_label(app.ui_locale, app.mode),
        Style::default().fg(mode_color),
    ));
    // Permission is safety state, not optional chrome. Compact terminals shed
    // auxiliary detail, but keep mode and the effective posture.
    left.push(Span::styled(FIELD_JOIN, dim));
    left.push(Span::styled(
        permission_label(app),
        Style::default().fg(permission_color),
    ));
    let scope_notice = filesystem_scope_notice(app);
    if let Some(scope) = scope_notice.clone() {
        left.push(Span::styled(FIELD_JOIN, dim));
        left.push(Span::styled(scope, Style::default().fg(permission_color)));
    }
    let posture = left.clone();
    // Active-goal chip (#39): the ocean shell has no sidebar, so the topbar
    // is the only always-on surface where a goal set via `create_goal` can
    // live. Objective truncated to a fixed budget; terminal goals render
    // nothing. The cramped-layout rebuild below keeps the chip in `suffix`.
    let goal_chip =
        crate::tui::footer_ui::active_goal_chip_state(app).map(|(objective, paused)| {
            let budget = if paused { 22 } else { 26 };
            let flat = objective.trim().replace(['\n', '\r'], " ");
            let text = if paused {
                format!("goal paused {}", truncate_to_width(&flat, budget))
            } else {
                format!("goal {}", truncate_to_width(&flat, budget))
            };
            let color = if paused {
                ChromeInk::Attention.color(&app.ui_theme)
            } else {
                ChromeInk::Active.color(&app.ui_theme)
            };
            (text, color)
        });
    if let Some((text, color)) = &goal_chip {
        left.push(Span::raw(GROUP_GAP));
        left.push(Span::styled(
            text.clone(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
    }
    // Workflow-run chip (#5040): the same `WorkflowPanel::top_bar_chip` the
    // classic header shows, so a collapsed run stays visible on the ocean
    // shell too. No workflow panel means no chip. The cramped-layout rebuild
    // below keeps the chip in `suffix` alongside the goal chip.
    let workflow_chip = app.workflow_panel.as_ref().map(|panel| {
        let ink = if matches!(
            panel.lifecycle,
            crate::tui::widgets::workflow_panel::WorkflowPanelLifecycle::Degraded
        ) {
            ChromeInk::Attention
        } else {
            ChromeInk::Info
        };
        (panel.top_bar_chip(), ink.color(&app.ui_theme))
    });
    if let Some((text, color)) = &workflow_chip {
        left.push(Span::raw(GROUP_GAP));
        left.push(Span::styled(
            text.clone(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
    }
    // Update-available chip (#14): a quiet, persistent affordance set once by
    // the startup version check. Gets the workflow chip's treatment: last in
    // the left cluster, the route label yields its budget first, and the chip
    // drops cleanly when even a minimal chip cannot fit — never a modal,
    // never mid-chip clipping.
    let update_chip = app
        .update_available
        .as_ref()
        .map(|label| (label.clone(), ChromeInk::Attention.color(&app.ui_theme)));
    if let Some((text, color)) = &update_chip {
        left.push(Span::raw(GROUP_GAP));
        left.push(Span::styled(
            text.clone(),
            Style::default().fg(*color).add_modifier(Modifier::BOLD),
        ));
    }

    let context_meter = header_context_meter(app, tier);
    let token_breakdown = (tier != ShellTier::Compact)
        .then(|| session_token_breakdown(app))
        .flatten();
    // Cached repository/worktree status only — never probe from the render path.
    // Background refresh is scheduled from the event loop / idle ticks.
    let git_label = crate::tui::git_status::chrome_label(git_status).map(|label| {
        let max_width = match tier {
            ShellTier::Compact => 24,
            ShellTier::Normal => 36,
            ShellTier::Wide => 52,
        };
        Span::styled(
            truncate_to_width(&label, max_width),
            header_fg(app, crate::tui::git_status::chrome_ink()),
        )
    });

    // Baseline right-hand chrome: git, then the context meter.
    //
    // The build version used to close this cluster. It was already the first
    // thing the header sacrificed — present only on `Wide`, gone below 110
    // columns — which is the layout admitting it was never load-bearing. It is
    // a fact you check deliberately (`codewhale --version`, `codewhale
    // doctor`, the launch screen) exactly once, and the half of it that *is*
    // worth reading mid-session — "your build is stale" — already has its own
    // chip on the left. Fifteen columns of the primary chrome on every screen
    // forever bought a numeral nobody was reading.
    let mut right = Vec::new();
    if let Some(git_label) = git_label.clone() {
        push_chrome(&mut right, git_label);
    }
    if let Some(context_meter) = context_meter.clone() {
        push_chrome(&mut right, context_meter);
    }

    // The posture lockup is the header's floor: mark, mode, permission, and a
    // deviating filesystem scope never yield their columns to anything on the
    // right. It is measured, not re-derived, so the floor cannot drift away
    // from what actually gets drawn.
    let minimum_left_width = span_width(&posture);
    let available = usize::from(area.width);
    // The optional token breakdown is the only elidable element: it is added
    // between the git label and the context meter when the terminal is wide
    // enough to keep the whole baseline plus the guaranteed-left minimum.
    if let Some(token_breakdown) = token_breakdown {
        let mut enhanced_right = Vec::new();
        if let Some(git_label) = git_label.clone() {
            push_chrome(&mut enhanced_right, git_label);
        }
        push_chrome(&mut enhanced_right, token_breakdown);
        if let Some(context_meter) = context_meter.clone() {
            push_chrome(&mut enhanced_right, context_meter);
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
        // Cramped: keep the posture lockup exactly as composed and re-hang the
        // chips behind it. Rebuilding the lockup by hand here is how the two
        // passes used to disagree about what the header guarantees.
        let mut compact_left = posture.clone();
        // The goal chip survives cramped layouts too — it is operator state,
        // not decoration. The route label yields its budget first (down to
        // nothing, as it always has); below that the goal itself truncates,
        // and when even a minimal chip cannot fit it drops rather than
        // clipping mid-word (#39).
        let base_fixed = span_width(&compact_left);
        if let Some((text, color)) = &goal_chip {
            let goal_room = left_budget
                .saturating_sub(base_fixed)
                .saturating_sub(GROUP_GAP.len());
            if goal_room >= 8 {
                compact_left.push(Span::raw(GROUP_GAP));
                compact_left.push(Span::styled(
                    truncate_to_width(text, goal_room),
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                ));
            }
        }
        // The workflow chip (#5040) is operator state too, so it gets the
        // goal chip's treatment: whatever room remains after the chips ahead
        // of it, clean truncation, and a clean drop when even a minimal chip
        // cannot fit. The route label still yields its budget first.
        if let Some((text, color)) = &workflow_chip {
            let workflow_room = left_budget
                .saturating_sub(span_width(&compact_left))
                .saturating_sub(GROUP_GAP.len());
            if workflow_room >= 8 {
                compact_left.push(Span::raw(GROUP_GAP));
                compact_left.push(Span::styled(
                    truncate_to_width(text, workflow_room),
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                ));
            }
        }
        // The update chip (#14) gets the same treatment, last in line: it is
        // useful, but it yields to every piece of operator state ahead of it.
        if let Some((text, color)) = &update_chip {
            let update_room = left_budget
                .saturating_sub(span_width(&compact_left))
                .saturating_sub(GROUP_GAP.len());
            if update_room >= 8 {
                compact_left.push(Span::raw(GROUP_GAP));
                compact_left.push(Span::styled(
                    truncate_to_width(text, update_room),
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                ));
            }
        }
        left = compact_left;
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

/// The transcript rows the idle brand mark needs before it will draw at all.
///
/// Named so the *layout* can honour it before the frame is split. Anything that reserves rows above
/// the transcript must subtract against this constant rather than guess, or
/// the reservation and the render gate drift and the mark is evicted by
/// chrome that was sized without knowing the mark existed.
pub(crate) const AMBIENT_MIN_CHAT_HEIGHT: u16 = 16;
/// Companion column floor, same reasoning as [`AMBIENT_MIN_CHAT_HEIGHT`].
pub(crate) const AMBIENT_MIN_CHAT_WIDTH: u16 = 60;

/// Build the post-launch idle composition: brand, workspace context, and one
/// direct invitation. Commands stay in the command surface instead of reading
/// like onboarding homework.
///
/// Expressed in terms of the ambient floor constants so the layout rule that
/// reserves the rows and the gate that spends them cannot disagree. (The old
/// spelling also tested `height >= 14 && width >= 28`, which was dead: the
/// tier check already demands 16 rows and 60 columns.)
#[must_use]
pub(crate) fn empty_state_mark_visible(area: Rect) -> bool {
    area.height >= AMBIENT_MIN_CHAT_HEIGHT && area.width >= AMBIENT_MIN_CHAT_WIDTH
}

#[must_use]
pub(crate) fn decorative_shell_motion_enabled(app: &App) -> bool {
    app.motion_policy().allows_decorative()
        && !app.attention_hold_active()
        && app.onboarding == OnboardingState::None
        && !app.launch.visible
        && app.view_stack.is_empty()
}

/// Shorten a workspace path to its trailing components, marked with a leading
/// ellipsis so it reads as "somewhere above here" rather than as a real path.
fn shorten_workspace(workspace: &str, keep: usize) -> String {
    let sep = if workspace.contains('/') { '/' } else { '\\' };
    let parts: Vec<&str> = workspace.split(sep).filter(|p| !p.is_empty()).collect();
    if parts.len() <= keep {
        return workspace.to_string();
    }
    let tail = parts[parts.len() - keep..].join(&sep.to_string());
    let shortened = format!("…{sep}{tail}");
    // Only elide when it actually buys width. `~/code/app` -> `…/code/app` is
    // the same length and throws away the `~`, which carries more meaning than
    // the ellipsis does.
    if shortened.width() >= workspace.width() {
        return workspace.to_string();
    }
    shortened
}

/// Compose the empty-state caption so the caller's centering can survive.
///
/// This line sits between the wordmark and "What do you want to accomplish?",
/// and every other element of that block is centered. It used to be built at
/// full length and then handed to `truncate_to_width(.., width)`, which made it
/// exactly `width` wide — so the caller's `(width - context.width()) / 2` inset
/// evaluated to zero and the caption rendered flush-left, full-bleed, cutting
/// the composition in half. The clipping also destroyed the information: an
/// absolute path truncated mid-directory ("…/34267917-11f4-4d15-911a-…") tells
/// the reader nothing about where they are.
///
/// So the caption sheds detail rather than getting cut. In order of what goes
/// first: the MCP count, then the branch, then the leading path components. The
/// folder you are in is the last thing to go, because it is the only part a
/// person actually reads here.
///
/// One rule was added after watching it at 120 columns: the margin is
/// proportional, not a flat four. A flat four let a 114-column path "fit" a
/// 119-column lane, which put the centring inset back at two and reproduced
/// the full-bleed banner this function exists to prevent — the same failure,
/// arrived at from the other direction. A sixth of the lane, split either
/// side, means the caption is always visibly a caption.
fn empty_state_caption(
    workspace: &str,
    branch: &str,
    mcp_label: &str,
    mcp_count: usize,
    width: usize,
) -> String {
    // Leave a margin so the line is visibly inset rather than merely fitting,
    // and scale it, because "four columns" is only a margin at 60 columns.
    let budget = width.saturating_sub((width / 6).max(4)).max(8);
    let candidates = [
        format!("{workspace} · {branch} · {mcp_label} {mcp_count}"),
        format!("{workspace} · {branch}"),
        workspace.to_string(),
        format!("{} · {branch}", shorten_workspace(workspace, 2)),
        shorten_workspace(workspace, 2),
        shorten_workspace(workspace, 1),
    ];
    for candidate in &candidates {
        if candidate.width() <= budget {
            return candidate.clone();
        }
    }
    // Nothing fit: the last resort is the folder name alone, and the caller
    // still clamps. Better a bare name than a path clipped mid-component.
    shorten_workspace(workspace, 1)
}

pub fn empty_state_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    if area.width == 0 || area.height == 0 {
        return Vec::new();
    }
    let width = usize::from(area.width);
    let mut lines = vec![Line::from(""); usize::from(area.height / 4)];
    // The idle whale portrait that used to open this block was deleted per
    // the 2026-08-29 founder directive; the ambient empty-state surface
    // (wordmark, context caption, prompt) is not whale art and stays.

    let identity = crate::tui::workspace_context::identity_from_context(
        &app.workspace,
        app.workspace_context.as_deref(),
    );
    let workspace = crate::utils::display_path(&app.workspace);
    let branch = identity.branch.as_deref().map_or_else(
        || tr(app.ui_locale, MessageId::EmptyStateNoGit),
        |branch| Cow::Owned(branch.to_string()),
    );
    // Compact used to bypass the caption entirely and print the bare branch,
    // which in a plain folder rendered as the single centred word "no git" —
    // a whole row of the hero spent naming something that is not there. The
    // shedding ladder already degrades gracefully at any width, so every tier
    // now goes through it.
    let context = empty_state_caption(
        &workspace,
        &branch,
        tr(app.ui_locale, MessageId::EmptyStateMcpLabel).as_ref(),
        app.mcp_configured_count,
        width,
    );
    let brand = "Codewhale";
    let brand_inset = " ".repeat(width.saturating_sub(brand.width()) / 2);
    lines.push(Line::from(Span::styled(
        format!("{brand_inset}{brand}"),
        Style::default()
            .fg(app.ui_theme.text_body)
            .add_modifier(Modifier::BOLD),
    )));
    let context = truncate_to_width(&context, width);
    let inset = " ".repeat(width.saturating_sub(context.width()) / 2);
    lines.push(Line::from(Span::styled(
        format!("{inset}{context}"),
        Style::default().fg(app.ui_theme.text_soft),
    )));
    if area.height >= 4 {
        lines.push(Line::from(""));
        let prompt = tr(app.ui_locale, MessageId::EmptyStatePrompt);
        let prompt = truncate_to_width(prompt.as_ref(), width);
        let inset = " ".repeat(width.saturating_sub(prompt.width()) / 2);
        lines.push(Line::from(Span::styled(
            format!("{inset}{prompt}"),
            Style::default().fg(app.ui_theme.text_body),
        )));
    }
    lines
}

#[cfg(test)]
mod launch_contract_tests {
    use super::{
        LaunchAction, QUICK_ACTION_ROWS, apply_launch_hitboxes, handle_launch_key,
        tideline_startup_hitboxes,
    };
    use crate::localization::Locale;
    use crate::tui::app::LaunchState;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    fn launch_state() -> LaunchState {
        LaunchState {
            visible: true,
            selected: 0,
            worktree_input: None,
            status: None,
            workspace_session_count: 2,
            worktree_available: true,
            row_areas: Vec::new(),
            option_areas: Vec::new(),
            composer_focus: false,
            composer_area: None,
            send_area: None,
        }
    }

    #[test]
    fn launch_rows_are_seven_table_slots_with_the_quick_actions_painted() {
        // `row_areas` is the launch table's seven slots: the stage's three
        // quick-action rows land at [2, 4, 1] and the unreachable slots hold
        // zero rects that never hit-test. Re-derived from `startup_layout`
        // through the same hitbox path `frame.rs` runs after the paint.
        let mut launch = launch_state();
        let stage = Rect::new(0, 1, 80, 22); // the frame's stage slot at 80x24
        let hitboxes = tideline_startup_hitboxes(stage);
        apply_launch_hitboxes(&hitboxes, &mut launch);
        assert_eq!(launch.row_areas.len(), 7, "one slot per launch-table row");
        for (quick_index, slot) in QUICK_ACTION_ROWS.iter().enumerate() {
            let row = launch.row_areas[*slot];
            assert_eq!(
                row, hitboxes.actions[quick_index],
                "quick action {quick_index} owns table slot {slot}"
            );
            assert!(row.width > 0);
        }
        for slot in [0usize, 5, 6] {
            assert_eq!(
                launch.row_areas[slot].width, 0,
                "slot {slot} has no painted row on the stage"
            );
        }
        // The docked composer's hitboxes ride the same registry.
        assert!(launch.composer_area.is_some() && launch.send_area.is_some());
        assert_eq!(launch.option_areas.len(), 4, "four tiles at 80 columns");

        // The 40x12 floor: the stage slot is 10 rows — the three quick
        // actions keep their slots and the dock survives as its input row.
        let floor_stage = Rect::new(0, 1, 40, 10);
        let floor_hitboxes = tideline_startup_hitboxes(floor_stage);
        apply_launch_hitboxes(&floor_hitboxes, &mut launch);
        assert_eq!(launch.row_areas.len(), 7);
        assert!(
            launch.composer_area.is_some(),
            "the floor keeps the composer"
        );
    }

    #[test]
    fn selected_rows_and_direct_keys_dispatch_the_same_startup_actions() {
        let cases = [
            (
                0,
                KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
                LaunchAction::Connect,
            ),
            (
                1,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                LaunchAction::Resume,
            ),
            (
                2,
                KeyEvent::new(KeyCode::Char('w'), KeyModifiers::NONE),
                LaunchAction::NewSession,
            ),
            (
                4,
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE),
                LaunchAction::NewChat,
            ),
            (
                5,
                KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE),
                LaunchAction::Theme,
            ),
            (
                6,
                KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
                LaunchAction::Help,
            ),
        ];
        for (index, direct, expected) in cases {
            let mut selected = launch_state();
            selected.selected = index;
            assert_eq!(
                handle_launch_key(
                    &mut selected,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    Locale::En,
                ),
                expected
            );

            let mut shortcut = launch_state();
            assert_eq!(
                handle_launch_key(&mut shortcut, direct, Locale::En),
                expected
            );
            assert_eq!(shortcut.selected, index);
        }

        let mut worktree = launch_state();
        worktree.selected = 3;
        assert_eq!(
            handle_launch_key(
                &mut worktree,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::None
        );
        assert_eq!(worktree.worktree_input.as_deref(), Some(""));
    }

    #[test]
    fn changelog_and_quit_remain_keyboard_actions_without_claiming_a_row() {
        let mut launch = launch_state();
        launch.selected = 4;
        assert_eq!(
            handle_launch_key(
                &mut launch,
                KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::Changelog
        );
        assert_eq!(launch.selected, 4);
        assert_eq!(
            handle_launch_key(
                &mut launch,
                KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::Quit
        );
        assert_eq!(launch.selected, 4);
    }
}

#[cfg(test)]
mod launch_composer_tests {
    use super::{
        LaunchAction, LaunchComposerKey, apply_launch_hitboxes, handle_launch_composer_key,
        handle_launch_key, launch_composer_rows, render_launch_completion_popup,
        render_tideline_startup, tideline_startup_from_app, tideline_startup_hitboxes,
    };
    use crate::localization::{Locale, MessageId, tr};
    use crate::tui::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    /// The four supported TERMINAL sizes from the Tideline responsiveness
    /// contract, exercised by every test in this module. The startup stage
    /// is the terminal minus the topbar and the merged footer (two rows).
    const LAUNCH_SIZES: [(u16, u16); 4] = [(40, 12), (60, 16), (80, 24), (140, 40)];

    fn launch_app() -> App {
        let mut app = crate::test_support::test_app_with_options(
            crate::test_support::test_tui_options(std::env::temp_dir()),
        );
        app.onboarding = crate::tui::app::OnboardingState::None;
        app.low_motion = false;
        app.launch.visible = true;
        app
    }

    /// The frame's stage slot for one terminal size (spec §5b: topbar 1,
    /// stage Min(1), footer 1) — the rect `render_tideline_startup` owns.
    fn stage_for(width: u16, height: u16) -> Rect {
        Rect::new(0, 1, width, height.saturating_sub(2))
    }

    /// Render the launch surface's stage exactly as `frame.rs` does: the
    /// projected startup widget (hero, quick actions, option strip, docked
    /// composer), with the hitboxes applied as the frame applies them.
    fn render(app: &App, width: u16, height: u16) -> (Buffer, Rect) {
        let area = stage_for(width, height);
        let mut buf = Buffer::empty(area);
        let startup = tideline_startup_from_app(app);
        render_tideline_startup(area, &mut buf, &startup);
        let hitboxes = tideline_startup_hitboxes(area);
        let mut launch = app.launch.clone();
        apply_launch_hitboxes(&hitboxes, &mut launch);
        (buf, area)
    }

    #[test]
    fn caret_window_budgets_by_display_width_so_wide_drafts_keep_the_caret() {
        use unicode_width::UnicodeWidthStr;
        // 12 CJK characters = 24 display cells against a 9-column budget:
        // a character-count slice kept 8 CHARACTERS (16 cells) and pushed
        // the caret past the clip end (review finding 4).
        let line = "你好世界你好世界你好世界";
        let (before, after) = super::launch_caret_window(line, line.chars().count(), 9);
        assert!(
            before.width() <= 8,
            "before must fit its cell budget: {} cells",
            before.width()
        );
        assert!(before.width() + 1 + after.width() <= 9);
        assert!(
            !before.is_empty(),
            "the window keeps the widest tail that fits"
        );
        // ASCII behavior is unchanged: the trailing characters, nothing wider.
        let (ascii_before, ascii_after) = super::launch_caret_window("hello world", 11, 6);
        assert_eq!(ascii_before, "world");
        assert_eq!(ascii_after, "");
    }

    #[test]
    fn context_meter_hitbox_yields_to_the_posture_floor() {
        let mut app = launch_app();
        app.session.last_prompt_tokens = Some(1_000);
        // Wide header: the meter owns its right-edge columns.
        let wide = super::header_hitboxes(Rect::new(0, 0, 120, 1), &app);
        assert_eq!(wide.len(), 1, "wide header registers the meter hitbox");
        // Compact header: the posture lockup is the guaranteed floor and is
        // never truncated, so at narrow widths it can run into the meter's
        // columns — the hitbox must not claim cells the posture paints
        // (review finding 5).
        let narrow = super::header_hitboxes(Rect::new(0, 0, 16, 1), &app);
        assert!(
            narrow.is_empty(),
            "compact header must not claim overlapped cells"
        );
    }

    #[test]
    fn enter_applies_a_visible_slash_completion_instead_of_sending_the_prefix() {
        // #5698 review finding 1: the launch composer classified Enter as
        // Submit without consulting the completion menus, so `/mo` + Enter
        // sent the literal text instead of running `/model`.
        let mut app = launch_app();
        app.launch.composer_focus = true;
        app.input = "/mo".to_string();
        app.cursor_position = app.input.chars().count();
        let entries = crate::tui::slash_menu::visible_slash_menu_entries(&app, 1);
        assert!(
            !entries.is_empty(),
            "precondition: /mo must match at least one command"
        );
        let verdict =
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(verdict, LaunchComposerKey::Submit);
        let completed = app.input.clone();
        assert!(
            completed.starts_with('/')
                && completed != "/mo"
                && entries.iter().any(|e| {
                    e.name == completed.trim_end() || completed.starts_with(&format!("{}/", e.name))
                }),
            "Enter must apply the highlighted completion (matched {:?}), input now: {completed:?}",
            entries.first().map(|e| e.name.clone())
        );
    }

    #[test]
    fn completion_popup_paints_above_the_launch_composer() {
        // #5698 review finding 2: the menus were invisible on launch — the
        // frame returned before the ComposerWidget popup path ran. The
        // stage dock keeps that fix: the popup paints above the docked
        // input row, inside the stage.
        let app = launch_app();
        let area = stage_for(80, 24);
        let (input_y, _) = launch_composer_rows(area).unwrap();
        let entries = vec![crate::tui::widgets::SlashMenuEntry {
            name: "/model".to_string(),
            description: "Pick the model".to_string(),
            is_skill: false,
            alias_hint: None,
        }];
        let mut buf = Buffer::empty(area);
        let mut app = app;
        app.launch.composer_focus = true;
        let startup = tideline_startup_from_app(&app);
        render_tideline_startup(area, &mut buf, &startup);
        render_launch_completion_popup(area, &mut buf, &app, input_y, &entries, &[]);
        let popup_row = (area.y..area.y + input_y)
            .rev()
            .map(|y| {
                (area.x..area.x + area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .find(|line| line.contains("/model"))
            .expect("the completion menu must be visible above the composer");
        assert!(
            popup_row.contains("▸") || popup_row.contains('>') || popup_row.contains('*'),
            "the selected entry carries a selection marker: {popup_row:?}"
        );
    }

    /// Row `y` of `area` as text — `y` is area-relative.
    fn row_text(buf: &Buffer, area: Rect, y: u16) -> String {
        (area.x..area.x + area.width)
            .map(|x| buf[(x, area.y + y)].symbol().to_string())
            .collect()
    }

    /// Cell columns `from..to` of one row (byte-safe against wide glyphs).
    /// Cell columns `from..to` of row `y` (area-relative, byte-safe against
    /// wide glyphs).
    fn row_cells(buf: &Buffer, area: Rect, y: u16, from: u16, to: u16) -> String {
        (from..to.min(area.x + area.width))
            .map(|x| buf[(x, area.y + y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn composer_strip_docks_at_every_supported_size_without_displacing_choices() {
        for (width, height) in LAUNCH_SIZES {
            let mut app = launch_app();
            let stage = stage_for(width, height);
            let hitboxes = tideline_startup_hitboxes(stage);
            apply_launch_hitboxes(&hitboxes, &mut app.launch);
            let (input_y, hint_y) =
                launch_composer_rows(stage).expect("composer must fit at a supported size");

            let (buf, area) = render(&app, width, height);
            let input_row = row_text(&buf, area, input_y);
            assert!(
                input_row.contains('❯'),
                "{width}x{height}: composer row lacks its prompt anchor: {input_row:?}"
            );
            assert!(
                input_row.contains(&tr(Locale::En, MessageId::ComposerPlaceholder).into_owned()),
                "{width}x{height}: empty composer must show the shared placeholder: {input_row:?}"
            );

            // The launch table keeps all seven slots; the quick actions own
            // theirs and the tiles ride the option registry.
            assert_eq!(
                app.launch.row_areas.len(),
                7,
                "{width}x{height}: every startup choice must stay reachable"
            );
            assert!(
                app.launch.option_areas.len() >= 2,
                "{width}x{height}: the option strip keeps at least two tiles"
            );
            // Hitboxes mirror the rendered row, and send sits at its end.
            let composer = app.launch.composer_area.expect("composer hitbox");
            let send = app.launch.send_area.expect("send hitbox");
            assert!(
                composer.y <= area.y + input_y && area.y + input_y < composer.bottom(),
                "{width}x{height}: input row must live inside the focus surface"
            );
            assert!(
                composer.y <= send.y && send.y < composer.bottom(),
                "{width}x{height}: send target must live inside the focus surface"
            );
            assert!(send.right() <= composer.right());
            let expected_send = if send.width == 3 { "[↑]" } else { " ↑" };
            assert_eq!(
                row_cells(&buf, area, send.y - area.y, send.x, send.right()),
                expected_send,
                "{width}x{height}: send hitbox must cover the rendered send glyph"
            );
            assert!(
                input_y < hint_y && hint_y <= area.height,
                "{width}x{height}: composer rows must stack inside the stage"
            );
        }
    }

    #[test]
    fn unfocused_composer_advertises_tab_and_focused_composer_advertises_submit() {
        let mut app = launch_app();
        let (buf, area) = render(&app, 80, 24);
        let (input_y, hint_y) = launch_composer_rows(stage_for(80, 24)).unwrap();
        assert!(
            row_text(&buf, area, hint_y)
                .contains(&tr(Locale::En, MessageId::LaunchComposerFocusHint).into_owned()),
            "unfocused hint row must show how to start typing"
        );
        assert!(!row_text(&buf, area, input_y).contains('▌'));

        // Tab is the keyboard path into the composer.
        assert_eq!(
            handle_launch_key(
                &mut app.launch,
                KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::None
        );
        assert!(app.launch.composer_focus);
        let (buf, area) = render(&app, 80, 24);
        assert!(row_text(&buf, area, input_y).contains('▌'));
        let _ = hint_y;
        assert!(
            row_text(&buf, area, hint_y)
                .contains(&tr(Locale::En, MessageId::LaunchComposerHint).into_owned()),
            "focused hint row must explain Enter/Esc"
        );

        // Esc hands focus back without touching the composed text.
        app.insert_char('h');
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            LaunchComposerKey::Blur
        );
        assert!(!app.launch.composer_focus);
        assert_eq!(app.input, "h");
    }

    /// Mirror of the event loop's fall-through: an admitted editing key is
    /// answered by the conversation composer authority — the router never
    /// performs the edit itself, so the test performs exactly the shared
    /// call the conversation match makes.
    fn type_char(app: &mut App, ch: char) {
        assert_eq!(
            handle_launch_composer_key(app, KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)),
            LaunchComposerKey::ComposerAuthority
        );
        app.insert_char(ch);
    }

    #[test]
    fn editing_keys_are_omitted_to_the_composer_authority_not_reimplemented() {
        let mut app = launch_app();
        app.launch.composer_focus = true;

        // Text and caret keys are only admitted here; the shared App edit
        // methods the conversation match calls produce the edit.
        type_char(&mut app, 'h');
        type_char(&mut app, 'i');
        assert_eq!(app.input, "hi");
        assert_eq!(
            handle_launch_composer_key(
                &mut app,
                KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)
            ),
            LaunchComposerKey::ComposerAuthority
        );
        app.delete_char();
        assert_eq!(app.input, "h");

        // A direct startup shortcut letter types into the composer now…
        type_char(&mut app, 'p');
        assert_eq!(app.input, "hp");

        // …and word motion is composer-owned too: Alt+B moves a whole word
        // back through the exact shared helper the conversation composer
        // uses, instead of blurring or reaching the startup menu.
        for ch in " one two".chars() {
            type_char(&mut app, ch);
        }
        assert_eq!(app.input, "hp one two");
        assert_eq!(app.cursor_position, 10);
        let alt_b = KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT);
        assert_eq!(
            handle_launch_composer_key(&mut app, alt_b),
            LaunchComposerKey::ComposerAuthority
        );
        assert!(crate::tui::composer_ui::handle_composer_alt_word_motion_key(&mut app, alt_b));
        assert_eq!(
            app.cursor_position, 7,
            "Alt+B must move a word back inside the focused composer"
        );
        assert!(app.launch.composer_focus);

        // …while the startup menu's chords stay menu-owned.
        assert_eq!(
            handle_launch_composer_key(
                &mut app,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL)
            ),
            LaunchComposerKey::MenuChord
        );
        assert!(app.launch.composer_focus);
        assert_eq!(
            handle_launch_key(
                &mut app.launch,
                KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
                Locale::En,
            ),
            LaunchAction::Resume
        );
        for (code, modifiers) in [
            (KeyCode::Char('n'), KeyModifiers::CONTROL),
            (KeyCode::Char('l'), KeyModifiers::CONTROL),
            (KeyCode::Char('q'), KeyModifiers::CONTROL),
            (KeyCode::F(1), KeyModifiers::NONE),
        ] {
            assert_eq!(
                handle_launch_composer_key(&mut app, KeyEvent::new(code, modifiers)),
                LaunchComposerKey::MenuChord,
                "{code:?} must stay menu-owned while the composer holds focus"
            );
        }

        // Up/Down leave the composer and then move the menu selection. The
        // Ctrl+R shortcut above moved it to its row; start from the top.
        app.launch.selected = 0;
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
            LaunchComposerKey::BlurToMenu
        );
        assert!(!app.launch.composer_focus);
        assert_eq!(
            handle_launch_key(
                &mut app.launch,
                KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::None
        );
        assert_eq!(app.launch.selected, 1);
    }

    #[test]
    fn completion_menus_stay_composer_owned_instead_of_blurring_to_the_menu() {
        let mut app = launch_app();
        app.launch.composer_focus = true;
        type_char(&mut app, '/');
        type_char(&mut app, 'm');
        type_char(&mut app, 'o');
        assert!(
            !crate::tui::slash_menu::visible_slash_menu_entries(&app, 1).is_empty(),
            "precondition: /mo must open the command completion menu"
        );
        // The completion menu is composer-owned: plain Up must reach the
        // conversation authority for entry navigation, not blur to rows.
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE)),
            LaunchComposerKey::ComposerAuthority
        );
        assert!(app.launch.composer_focus);
    }

    #[test]
    fn composer_enter_probe_mirrors_the_real_enter_without_mutating() {
        let mut app = launch_app();
        app.launch.composer_focus = true;
        assert!(
            !app.composer_enter_would_submit(),
            "an empty composer must not submit"
        );

        app.input = "  ".to_string();
        assert!(
            !app.composer_enter_would_submit(),
            "a whitespace-only draft is not a submit"
        );

        app.input = "ship it".to_string();
        app.cursor_position = 7;
        assert!(app.composer_enter_would_submit());
        assert_eq!(app.input, "ship it", "the probe must not consume the draft");
        assert!(app.launch.composer_focus);
    }

    #[test]
    fn enter_submits_through_the_real_composer_path() {
        let mut app = launch_app();
        app.launch.composer_focus = true;
        for ch in "hello world".chars() {
            type_char(&mut app, ch);
        }
        assert_eq!(app.input, "hello world");
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            LaunchComposerKey::Submit
        );
        // The event loop feeds this exact call into the normal dispatch
        // path after the launch session begins; the composer owns the text.
        assert_eq!(app.handle_composer_enter().as_deref(), Some("hello world"));
        assert!(app.input.is_empty());

        // Enter on an empty composer only returns focus to the menu.
        let mut empty = launch_app();
        empty.launch.composer_focus = true;
        assert_eq!(
            handle_launch_composer_key(
                &mut empty,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
            ),
            LaunchComposerKey::Blur
        );
        assert!(!empty.launch.composer_focus);
    }

    #[test]
    fn shift_enter_keeps_a_real_newline_in_the_composer_state() {
        let mut app = launch_app();
        app.launch.composer_focus = true;
        type_char(&mut app, 'a');
        type_char(&mut app, 'b');
        // Shift+Enter is a newline chord, not a submit: the router omits it
        // to the composer authority, whose newline arm owns the insertion.
        assert_eq!(
            handle_launch_composer_key(
                &mut app,
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)
            ),
            LaunchComposerKey::ComposerAuthority
        );
        assert!(crate::tui::composer_ui::is_composer_newline_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
            app.composer_multiline_mode
        ));
        app.insert_char('\n');
        type_char(&mut app, 'c');
        assert_eq!(app.input, "ab\nc");

        // The single-row projection truthfully shows the caret's line.
        let (buf, area) = render(&app, 80, 24);
        let (input_y, _) = launch_composer_rows(stage_for(80, 24)).unwrap();
        assert!(
            row_text(&buf, area, input_y).contains("c▌"),
            "composer row must show the caret's line, not the first line"
        );
    }

    #[test]
    fn floor_keeps_every_choice_and_a_usable_composer_row() {
        // The 40x12 floor's stage is 10 rows: the dock sheds to its input
        // row, the quick actions keep their table slots, and the composer
        // never disappears (the data — caret, draft — survives; only the
        // hint surface sheds, and it returns one tier up).
        let mut app = launch_app();
        app.launch.composer_focus = true;
        let stage = stage_for(40, 12);
        let hitboxes = tideline_startup_hitboxes(stage);
        apply_launch_hitboxes(&hitboxes, &mut app.launch);
        assert_eq!(
            app.launch.row_areas.len(),
            7,
            "the supported 40x12 floor must retain every startup choice"
        );
        assert!(app.launch.composer_area.is_some() && app.launch.send_area.is_some());
        let (buf, area) = render(&app, 40, 12);
        let (input_y, hint_y) = launch_composer_rows(stage).unwrap();
        assert_eq!(input_y, 9, "the dock's one row is the stage's last");
        assert_eq!(hint_y, area.height, "no second row to share at this tier");
        let input_row = row_text(&buf, area, input_y);
        assert!(
            input_row.contains('❯') && input_row.contains('▌'),
            "focused floor composer keeps its anchors and caret: {input_row:?}"
        );

        // One tier up (a 22-row terminal, stage 20, dock 2) the hint shares
        // the dock's second row — the classic compact tier's semantic.
        let (buf, area) = render(&app, 80, 22);
        let stage22 = stage_for(80, 22);
        let (input_y, hint_y) = launch_composer_rows(stage22).unwrap();
        assert_eq!(hint_y, input_y + 1, "the two-row dock shares its row");
        let hint_row = row_text(&buf, area, hint_y);
        assert!(
            hint_row.trim_start().starts_with(
                &tr(Locale::En, MessageId::LaunchComposerHint)
                    .chars()
                    .take(20)
                    .collect::<String>()
            ),
            "focused compact dock must carry the composer hint: {hint_row:?}"
        );
    }

    #[test]
    fn floor_blurred_composer_keeps_the_draft_and_the_next_tier_advertises_refocus() {
        // Esc keeps the draft but hands focus back. At the floor the dock is
        // one row — the draft itself is the surface that must survive; the
        // how-to-refocus copy returns with the hint row one tier up.
        let mut app = launch_app();
        app.launch.composer_focus = true;
        type_char(&mut app, 'd');
        type_char(&mut app, 'r');
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            LaunchComposerKey::Blur
        );
        assert!(!app.launch.composer_focus);
        let (buf, area) = render(&app, 40, 12);
        let (input_y, _) = launch_composer_rows(stage_for(40, 12)).unwrap();
        let input_row = row_text(&buf, area, input_y);
        assert!(
            input_row.contains("dr"),
            "the blurred composer must keep the draft: {input_row:?}"
        );

        let (buf, area) = render(&app, 80, 22);
        let (_, hint_y) = launch_composer_rows(stage_for(80, 22)).unwrap();
        let hint_row = row_text(&buf, area, hint_y);
        assert!(
            hint_row.contains(&tr(Locale::En, MessageId::LaunchComposerFocusHint).into_owned()),
            "blurred compact dock must still say how to refocus: {hint_row:?}"
        );
    }
}

#[cfg(test)]
mod empty_state_caption_tests {
    use super::{empty_state_caption, shorten_workspace};
    use unicode_width::UnicodeWidthStr;

    const DEEP: &str = "/private/tmp/claude-501/-Volumes-VIXinSSD-CW-codewhale/34267917-11f4-4d15-911a-2a8acd5c49e1/scratchpad/surface/ws2";

    #[test]
    fn caption_stays_narrow_enough_to_actually_centre() {
        // The caller centres this line with `(width - caption.width()) / 2`.
        // Building it at full length and truncating to `width` made that inset
        // zero, so the caption rendered flush-left and full-bleed straight
        // through the centred whale/wordmark/prompt composition.
        for width in [60usize, 80, 100, 120] {
            let caption = empty_state_caption(DEEP, "no git", "MCP", 0, width);
            assert!(
                caption.width() <= width,
                "width {width}: caption {caption:?} overflows the lane",
            );
            assert!(
                width.saturating_sub(caption.width()) / 2 > 0,
                "width {width}: caption {caption:?} would render flush-left",
            );
        }
    }

    #[test]
    fn caption_keeps_the_folder_you_are_standing_in() {
        let long = "/a/very/deeply/nested/checkout/somewhere/far/away/myproject";
        for width in [40usize, 60, 80, 120] {
            let caption = empty_state_caption(long, "main", "MCP", 2, width);
            assert!(
                caption.contains("myproject"),
                "width {width}: {caption:?} dropped the current folder",
            );
        }
    }

    #[test]
    fn caption_sheds_the_least_important_detail_first() {
        let ws = "~/code/app";
        let wide = empty_state_caption(ws, "main", "MCP", 3, 120);
        assert!(wide.contains("MCP 3") && wide.contains("main") && wide.contains(ws));

        let mid = empty_state_caption(ws, "main", "MCP", 3, 24);
        assert!(
            !mid.contains("MCP"),
            "{mid:?} should shed the MCP count first"
        );
        assert!(mid.contains("main"), "{mid:?} should still name the branch");

        let tight = empty_state_caption(ws, "main", "MCP", 3, 16);
        assert!(
            tight.contains("app"),
            "{tight:?} should still name the folder"
        );
    }

    #[test]
    fn elision_lands_on_a_separator_not_mid_component() {
        // The old line ended in an ellipsis mid-directory
        // ("…/34267917-11f4-4d15-911a-"), which told the reader nothing.
        let caption = empty_state_caption(DEEP, "no git", "MCP", 0, 60);
        assert!(
            !caption.contains("2a8acd5c49e1"),
            "{caption:?} clipped mid-component"
        );
        if caption.starts_with('…') {
            assert!(
                caption.starts_with("…/"),
                "elision must land on a separator: {caption:?}",
            );
        }
    }

    #[test]
    fn caption_margin_scales_so_it_is_always_visibly_a_caption() {
        // The flat four-column margin only looked like a margin at 60 columns.
        // At 119 it let a 114-column path through with an inset of two — a
        // full-bleed banner cutting the centred composition in half, which is
        // the exact failure the shedding ladder exists to prevent.
        for width in [40usize, 60, 80, 100, 119, 120, 200] {
            for workspace in [DEEP, "/a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/project"] {
                let caption = empty_state_caption(workspace, "main", "MCP", 2, width);
                let inset = width.saturating_sub(caption.width()) / 2;
                assert!(
                    inset * 12 >= width,
                    "width {width}: caption {caption:?} insets by only {inset}",
                );
            }
        }
    }

    #[test]
    fn shorten_workspace_is_a_no_op_when_it_already_fits() {
        assert_eq!(shorten_workspace("~/code/app", 2), "~/code/app".to_string());
        assert_eq!(shorten_workspace("app", 2), "app".to_string());
    }
}

#[cfg(test)]
mod header_tests {
    use super::{
        FIELD_JOIN, GROUP_GAP, filesystem_scope_notice, header_hitboxes,
        render_header_with_git_status,
    };
    use crate::palette::ChromeInk;
    use crate::tui::app::{App, AppMode};
    use crate::tui::approval::ApprovalMode;
    use crate::tui::widgets::workflow_panel::{WorkflowPanel, WorkflowPanelLifecycle};
    use ratatui::{buffer::Buffer, layout::Rect};

    fn app() -> App {
        let mut app = crate::test_support::test_app_with_options(
            crate::test_support::test_tui_options(std::env::temp_dir()),
        );
        // Enforcement present, so the scope chip reflects the policy rather
        // than the host's missing backend.
        app.sandbox_backend = Some(crate::sandbox::SandboxType::None);
        app.mode = AppMode::Agent;
        app.approval_mode = ApprovalMode::Suggest;
        app
    }

    fn header_line(app: &App, width: u16) -> String {
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        render_header_with_git_status(
            area,
            &mut buf,
            app,
            &crate::tui::git_status::GitStatusSnapshot::default(),
        );
        (0..width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    #[test]
    fn default_posture_spends_no_columns_on_the_expected_scope() {
        // `files: workspace` used to be printed on every frame of every
        // session: seventeen columns of the primary chrome restating the
        // default. A notice that never turns off cannot warn.
        let app = app();
        assert!(filesystem_scope_notice(&app).is_none());
        let line = header_line(&app, 120);
        assert!(!line.contains("files:"), "{line:?}");
        assert!(line.starts_with("Codewhale"), "{line:?}");
        assert!(line.contains("work"), "{line:?}");
        assert!(line.contains("ask"), "{line:?}");
    }

    #[test]
    fn a_deviating_scope_still_takes_the_header() {
        // The chip exists for exactly this: tool-approval "Full Access" being
        // read as unrestricted disk writes. Folding the default away is what
        // makes this one land.
        let mut app = app();
        app.approval_mode = ApprovalMode::Bypass;
        app.configured_sandbox_mode = Some("danger-full-access".to_string());
        let notice = filesystem_scope_notice(&app).expect("full disk must be stated");
        assert_eq!(notice, "files: full disk");
        assert!(header_line(&app, 120).contains("files: full disk"));
    }

    #[test]
    fn full_access_never_stands_alone_without_its_scope() {
        // Bypass clamped to workspace-write: the permission chip says
        // "Full Access" while writes are in fact confined. That pairing is the
        // exact misreading the scope chip exists to prevent, so the chip must
        // speak even though workspace-write is otherwise the quiet default.
        let mut full = app();
        full.approval_mode = ApprovalMode::Bypass;
        full.configured_sandbox_mode = Some("workspace-write".to_string());
        let notice = filesystem_scope_notice(&full)
            .expect("Full Access must never appear without a scope beside it");
        assert_eq!(notice, "files: workspace");
        let line = header_line(&full, 120);
        assert!(line.contains("files: workspace"), "{line:?}");

        // And the default posture still stays quiet.
        let mut quiet = app();
        quiet.approval_mode = ApprovalMode::Suggest;
        quiet.configured_sandbox_mode = Some("workspace-write".to_string());
        assert!(filesystem_scope_notice(&quiet).is_none());
    }

    #[test]
    fn plan_mode_does_not_say_read_only_twice() {
        let mut app = app();
        app.mode = AppMode::Plan;
        assert!(filesystem_scope_notice(&app).is_none());
        let line = header_line(&app, 120);
        assert!(line.contains("read only"), "{line:?}");
        assert!(!line.contains("files: read-only"), "{line:?}");
    }

    #[test]
    fn the_build_version_is_not_permanent_chrome() {
        // It was already `Wide`-only, which is the layout admitting it was
        // never load-bearing; `codewhale --version`, `codewhale doctor` and
        // the launch screen are where a version is actually looked up, and
        // the half worth reading mid-session is the update chip.
        let app = app();
        for width in [60u16, 80, 120, 200] {
            let line = header_line(&app, width);
            assert!(
                !line.contains(concat!("v", env!("CODEWHALE_BUILD_VERSION"))),
                "width {width}: {line:?}",
            );
        }
    }

    #[test]
    fn chips_are_separated_from_posture_by_a_wider_gap_than_the_posture_join() {
        // One weight per meaning: `" · "` binds words into one phrase, the
        // group gap stands between whole facts. If a goal chip hangs off the
        // same dotted separator that joins mode to permission, the header is
        // an undifferentiated list again.
        let mut app = app();
        app.update_available = Some("update 0.9.11".to_string());
        let line = header_line(&app, 120);
        assert!(
            line.contains(&format!("ask{GROUP_GAP}update 0.9.11")),
            "{line:?}",
        );
        assert!(line.contains(&format!("work{FIELD_JOIN}ask")), "{line:?}");
        assert!(
            unicode_width::UnicodeWidthStr::width(GROUP_GAP)
                > unicode_width::UnicodeWidthStr::width(FIELD_JOIN),
            "the group gap must out-space the phrase join or nothing groups",
        );
    }

    #[test]
    fn collapsed_degraded_workflow_chip_uses_attention_ink() {
        let mut app = app();
        let mut panel = WorkflowPanel::new("workflow-partial", "review release", 1_000);
        panel.lifecycle = WorkflowPanelLifecycle::Degraded;
        panel.expanded = false;
        panel.completed_at_ms = Some(2_000);
        app.workflow_panel = Some(panel);

        let width = 200;
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        render_header_with_git_status(
            area,
            &mut buf,
            &app,
            &crate::tui::git_status::GitStatusSnapshot::default(),
        );
        let text = (0..width).map(|x| buf[(x, 0)].symbol()).collect::<String>();
        let start = text.find("wf degraded").expect("degraded workflow chip");
        let expected = ChromeInk::Attention.color(&app.ui_theme);
        for x in start..start + "wf degraded".len() {
            assert_eq!(
                buf[(x as u16, 0)].fg,
                expected,
                "collapsed degraded chip must stay amber at column {x}: {text:?}"
            );
        }
    }

    #[test]
    fn the_context_meter_states_its_percentage_and_registers_an_inspector_target() {
        // The percentage is the direct operator question ("how full am I?").
        // Fraction remains the auditable fact and the bar is the glance.
        let mut app = app();
        app.session.total_input_tokens = 3_000;
        let line = header_line(&app, 120);
        if line.contains('▱') || line.contains('▰') {
            assert!(!line.contains('['), "{line:?}");
            assert!(line.contains("context"), "{line:?}");
            assert!(line.contains('%'), "{line:?}");
            let hitboxes = header_hitboxes(Rect::new(0, 0, 120, 1), &app);
            assert_eq!(hitboxes.len(), 1);
            assert_eq!(hitboxes[0].area.right(), 120);
            assert_eq!(
                hitboxes[0].target,
                crate::tui::app::HeaderActionTarget::InspectContext
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tideline startup stage — hero, quick actions, option strip (spec §5a
// components "Hero (startup)", "Quick actions", "Option strip"; §5b startup
// layout contract; golden `startup_{w}x{h}`).
//
// Landed 2026-08-29: the startup stage is the launch screen's body inside
// the Tideline shell (`ui/frame.rs` renders topbar → this stage → the merged
// footer). It stays a pure, deterministic widget fed injected facts
// (`LaunchState`/`workspace_session_count`/provider state are projected by
// the caller via `tideline_startup_from_app`), proven against golden
// buffers. Cell rules per spec §2: one glyph per action with declared ASCII
// fallbacks; the wave rules are static `Span`s; semantic ink only.

use ratatui::layout::{Constraint, Layout};

use crate::palette::UiTheme;

/// Static wave rule between the hero and the quick actions (spec §5b). Dim,
/// never animated — decoration is opt-in and this is not decoration that
/// carries state.
/// How long the hero mark takes to surface.
const MARK_SURFACE_MS: u128 = 640;

/// Rows the hero owes its type block: heading + subtitle. The mark is only
/// drawn when it fits *above* these, so the words never lose to the picture.
const HERO_TEXT_ROWS: u16 = 2;
/// One blank row between the mark and the heading. Without it the fluke's
/// stem and the cap-height of the heading collide into one shape.
const HERO_MARK_GAP: u16 = 1;

const WAVE_RULE: &str = "⋯ ∼∼∼ ⋯";

/// One QUICK ACTIONS row: icon · label · description · command + `›`.
///
/// The `disabled` projection is the caller's (provider state, session
/// count); the widget only renders it dimmer and never invents availability.
#[derive(Debug, Clone)]
pub struct TidelineQuickAction {
    pub icon: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub command: &'static str,
    pub disabled: bool,
}

impl TidelineQuickAction {
    /// The approved startup screen's three rows. `provider_ready` gates the
    /// chat-only row (no model — spec states the disabled state), and a
    /// workspace with zero saved sessions gates resume.
    #[must_use]
    pub fn approved_set(provider_ready: bool, session_count: usize) -> Vec<Self> {
        vec![
            Self {
                icon: "⌁",
                label: "New session",
                description: "start a fresh agent run in this workspace",
                command: "Enter",
                disabled: false,
            },
            Self {
                icon: "◌",
                label: "Chat only",
                description: "plan and converse without touching the repo",
                command: "C",
                disabled: !provider_ready,
            },
            Self {
                icon: "↺",
                label: "Resume last",
                description: "pick up a saved session where it ended",
                command: "Ctrl+R",
                disabled: session_count == 0,
            },
        ]
    }
}

/// One option-strip tile: icon + label over its key (spec §5b, 4 columns).
///
/// Every printed key is a real dispatch on this branch: `Ctrl+N` and `C` go
/// through `handle_launch_key`'s direct-key table; `F1` and `F2` are the
/// shell-global help and settings routes (`shell_key_routing`), which stay
/// live on the launch screen. No tile advertises a key that does nothing.
#[derive(Debug, Clone)]
pub struct TidelineOption {
    pub icon: &'static str,
    pub label: &'static str,
    pub key: &'static str,
}

impl TidelineOption {
    /// The approved four: New worktree / Chat only / Theme / Help. Every
    /// printed key is a real dispatch through the launch table (main's
    /// #5698 input model): Ctrl+N/C/T/F1 are the table's direct keys for
    /// rows 3/4/5/6, the same code a tile click takes. The shell-global
    /// F2 settings route stays live alongside — it simply is not the
    /// tile's advertised key now that the table's `T` is.
    #[must_use]
    pub fn approved_set() -> Vec<Self> {
        vec![
            Self {
                icon: "⑂",
                label: "New worktree",
                key: "Ctrl+N",
            },
            Self {
                icon: "◌",
                label: "Chat only",
                key: "C",
            },
            Self {
                icon: "◐",
                label: "Theme",
                key: "T",
            },
            Self {
                icon: "?",
                label: "Help",
                key: "F1",
            },
        ]
    }
}

/// What the caller owes the startup stage. Everything injectable so renders
/// stay deterministic for golden buffers (spec §5a data sources:
/// `LaunchState`, `workspace_session_count`, provider state).
pub struct TidelineStartup<'a> {
    pub theme: &'a UiTheme,
    /// Locale used by compact option labels as well as injected composer copy.
    pub locale: Locale,
    /// `workspace_session_count > 0` — the hero subtitle and resume row read
    /// differently for a returning workspace (spec §5a "first-run vs
    /// returning").
    pub session_count: usize,
    /// Provider configured — gates the chat-only rows.
    pub provider_ready: bool,
    /// Focused quick action, if one holds focus (keyboard parity with the
    /// launch table's rows — see `QUICK_ACTION_ROWS`). `None` when an
    /// option tile or an unshown table row holds the selection instead.
    pub selected_action: Option<usize>,
    /// Hovered quick action, if any (value ink brightens + underline).
    pub hovered_action: Option<usize>,
    /// Selected option-strip tile, if any (see `OPTION_TILE_ROWS`).
    pub selected_option: Option<usize>,
    /// The launch surface's one transient line — the worktree-name prompt or
    /// a launch status message — painted over the composer dock's last row.
    pub status_line: Option<String>,
    /// The docked pre-session composer's display projection (#5698's
    /// composer authority, re-docked below the option strip per §5b).
    pub composer: LaunchComposerDisplay<'a>,
    /// ASCII-safe / NO_COLOR mode: every glyph through `ascii_fallback`.
    pub ascii_safe: bool,
    /// How far the hero mark has surfaced, in `[0,1]`. Injected rather than
    /// read from a clock in here so golden buffers stay deterministic and so
    /// the reduced-motion path is the *same* drawing at its endpoint rather
    /// than a second, drift-prone still frame. Callers pass `1.0` for a
    /// settled mark.
    pub surface_progress: f32,
}

impl<'a> TidelineStartup<'a> {
    #[must_use]
    pub fn new(theme: &'a UiTheme, session_count: usize, provider_ready: bool) -> Self {
        Self {
            theme,
            locale: Locale::En,
            session_count,
            provider_ready,
            selected_action: Some(0),
            hovered_action: None,
            selected_option: None,
            status_line: None,
            composer: LaunchComposerDisplay::default(),
            ascii_safe: false,
            surface_progress: 1.0,
        }
    }

    /// Set the hero mark's surface progress (`0.0` submerged → `1.0` settled).
    #[must_use]
    pub fn surface_progress(mut self, progress: f32) -> Self {
        self.surface_progress = progress.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn status_line(mut self, line: Option<String>) -> Self {
        self.status_line = line;
        self
    }

    #[must_use]
    pub fn locale(mut self, locale: Locale) -> Self {
        self.locale = locale;
        self
    }

    #[must_use]
    pub fn composer(mut self, composer: LaunchComposerDisplay<'a>) -> Self {
        self.composer = composer;
        self
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    fn actions(&self) -> Vec<TidelineQuickAction> {
        TidelineQuickAction::approved_set(self.provider_ready, self.session_count)
    }

    fn options(&self) -> Vec<TidelineOption> {
        TidelineOption::approved_set()
    }

    fn sym(&self, glyph: &str) -> String {
        if !self.ascii_safe {
            return glyph.to_string();
        }
        if let Some(fb) = crate::tui::glyphs::ascii_fallback(glyph) {
            return fb.to_string();
        }
        glyph
            .chars()
            .map(|c| {
                crate::tui::glyphs::ascii_fallback(&c.to_string())
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            })
            .collect()
    }
}

fn chrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    chrome_style(theme, ink)
}

fn set_span(buf: &mut Buffer, x: u16, y: u16, span: &Span<'_>) {
    if let Ok(clamped) = span.content.width().try_into() {
        buf.set_span(x, y, span, clamped);
    }
}

fn centered(buf: &mut Buffer, area: Rect, y: u16, span: &Span<'_>) {
    let inset = (area.width.saturating_sub(span.content.width() as u16)) / 2;
    set_span(buf, area.x + inset, area.y + y, span);
}

/// The startup stage's shared row budget — render and hitboxes must agree,
/// so the constraint arithmetic lives here (spec §5b). Fixed bands shed as
/// the stage shrinks: the QUICK ACTIONS label row and its margin collapse
/// below 15 stage rows, then the static wave rules below 11; the hero
/// percentage and the strip's column count (never its rows) absorb the rest.
/// The pre-session composer docks in the spacer's bottom rows (§5b:
/// composer `Length(4)` incl. border) and sheds within itself before the
/// bands above ever move.
struct StartupLayout {
    hero: Rect,
    rule_a: Rect,
    quick: Rect,
    rule_b: Rect,
    strip: Rect,
    /// The docked pre-session composer: the spacer's bottom rows, at most
    /// four — `[input, hint, rule, prompt]` top to bottom (the prompt row
    /// is the stage's transient status line; the worktree-name prompt and
    /// launch status messages own it, painting over the hint).
    dock: Rect,
    /// Row within `quick` where the first action row paints.
    quick_rows_start: u16,
    /// Row within `strip` where labels start. Medium-height stages reclaim the
    /// purely decorative top padding so the compact composer can keep its hint.
    strip_content_start: u16,
    /// The four launch routes stay discoverable at every supported width;
    /// narrow stages shorten their labels instead of dropping actions.
    strip_columns: u16,
}

fn startup_layout(stage: Rect) -> StartupLayout {
    let quick_len: u16 = if stage.height >= 15 { 3 + 2 } else { 3 };
    let rule_len: u16 = if stage.height >= 11 { 1 } else { 0 };
    let strip_len: u16 = if (11..18).contains(&stage.height) {
        2
    } else {
        3
    };
    let [hero, rule_a, quick, rule_b, strip, tail] = Layout::vertical([
        Constraint::Percentage(38),
        Constraint::Length(rule_len),
        Constraint::Length(quick_len),
        Constraint::Length(rule_len),
        Constraint::Length(strip_len),
        Constraint::Min(1),
    ])
    .areas(stage);
    // The composer dock owns the tail's bottom rows — the spacer keeps
    // whatever the dock did not need. The dock never takes more than its
    // spec'd four rows, and never takes rows the fixed bands above already
    // claimed (the Min(1) guarantee).
    let dock_h = tail.height.min(4);
    let dock = Rect {
        y: tail.y + tail.height - dock_h,
        height: dock_h,
        ..tail
    };
    StartupLayout {
        hero,
        rule_a,
        quick,
        rule_b,
        strip,
        dock,
        quick_rows_start: quick.height.saturating_sub(3),
        strip_content_start: u16::from(strip_len > 2),
        strip_columns: 4,
    }
}

/// Paint the startup stage (spec §5b): hero → wave rule → QUICK ACTIONS →
/// wave rule → option strip → spacer. Deterministic; no clock, no motion.
pub fn render_tideline_startup(stage: Rect, buf: &mut Buffer, startup: &TidelineStartup<'_>) {
    if stage.width < 8 || stage.height < 5 {
        return;
    }
    let theme = startup.theme;
    let layout = startup_layout(stage);

    // Hero: fluke over heading + subtitle, as one centered block. The mark is
    // identity, not scenery, so it does not wait for the opt-in ocean
    // treatment; it lerps out of the theme's own surface colour instead.
    let mark_size = MarkSize::for_area(layout.hero, HERO_TEXT_ROWS);
    let mark_rows = mark_size.map_or(0, |size| size.cells().1 + HERO_MARK_GAP);
    let block_rows = mark_rows + HERO_TEXT_ROWS;
    let hero_row = layout.hero.height.saturating_sub(block_rows) / 2;
    if let Some(size) = mark_size {
        let mark_area = Rect {
            y: layout.hero.y + hero_row,
            height: layout.hero.height.saturating_sub(hero_row),
            ..layout.hero
        };
        crate::tui::mark::render_fluke(
            mark_area,
            buf,
            size,
            theme.accent_action,
            theme.surface_bg,
            startup.surface_progress,
            startup.ascii_safe,
        );
    }
    let hero_row = hero_row + mark_rows;
    let heading = "What are we working on?";
    centered(
        buf,
        layout.hero,
        hero_row,
        &Span::styled(
            heading,
            chrome(theme, ChromeInk::MetadataValue).add_modifier(Modifier::BOLD),
        ),
    );
    let subtitle = if startup.session_count > 0 {
        format!(
            "welcome back · {} saved {} in this workspace",
            startup.session_count,
            if startup.session_count == 1 {
                "session"
            } else {
                "sessions"
            },
        )
    } else {
        "type below, or pick a first move".to_string()
    };
    centered(
        buf,
        layout.hero,
        hero_row.saturating_add(1),
        &Span::styled(
            truncate_to_width(&subtitle, usize::from(layout.hero.width)),
            chrome(theme, ChromeInk::MetadataHint),
        ),
    );

    // Static wave rules.
    if layout.rule_a.height > 0 {
        let rule = startup.sym(WAVE_RULE);
        let rule_span = Span::styled(rule, chrome(theme, ChromeInk::MetadataDim));
        centered(buf, layout.rule_a, 0, &rule_span);
        centered(buf, layout.rule_b, 0, &rule_span);
    }

    // QUICK ACTIONS: label row + 3 rows of icon · label · description ·
    // command + `›`, right-aligned command. The label row is the first
    // thing to shed (§5b: identity of the band is its rows, not its title).
    if layout.quick_rows_start > 0 {
        set_span(
            buf,
            layout.quick.x + 2,
            layout.quick.y,
            &Span::styled(
                "QUICK ACTIONS",
                chrome(theme, ChromeInk::Metadata).add_modifier(Modifier::BOLD),
            ),
        );
    }
    let actions = startup.actions();
    let row_right = layout.quick.x + layout.quick.width.saturating_sub(2);
    for (index, action) in actions.iter().enumerate().take(3) {
        let y = layout.quick.y + layout.quick_rows_start + index as u16;
        if y >= layout.quick.bottom() {
            break;
        }
        let selected = startup.selected_action == Some(index);
        let hovered = startup.hovered_action == Some(index);
        let ink = if action.disabled {
            ChromeInk::MetadataDim
        } else if selected {
            ChromeInk::Identity
        } else {
            ChromeInk::MetadataValue
        };
        let mut style = chrome(theme, ink);
        if hovered && !action.disabled {
            style = style
                .add_modifier(Modifier::BOLD)
                .add_modifier(Modifier::UNDERLINED);
        }
        if selected && !action.disabled {
            style = style.add_modifier(Modifier::BOLD);
        }
        let marker = if selected { "▸ " } else { "  " };
        let mut row = format!(
            "{}{} {} — {}",
            marker,
            startup.sym(action.icon),
            action.label,
            action.description
        );
        if row.width() + 2 + action.command.width() + 2
            > layout.quick.width.saturating_sub(4) as usize
        {
            // Shed the description before the label: identity first.
            row = format!("{}{} {}", marker, startup.sym(action.icon), action.label);
        }
        let trailer = format!("{} ›", action.command);
        let trailer_w = trailer.width() as u16;
        set_span(
            buf,
            layout.quick.x + 2,
            y,
            &Span::styled(row, chrome(theme, ink)),
        );
        set_span(
            buf,
            row_right.saturating_sub(trailer_w),
            y,
            &Span::styled(startup.sym(&trailer), style),
        );
    }

    // Option strip: 4 columns × 2 rows (label over key). At narrow widths the
    // labels compact to one word; actions never disappear merely because the
    // terminal is small.
    let options = startup.options();
    let columns = layout.strip_columns;
    let column_w = layout.strip.width / columns.max(1);
    for (index, option) in options.iter().enumerate().take(usize::from(columns)) {
        let x = layout.strip.x + index as u16 * column_w;
        let selected = startup.selected_option == Some(index);
        let ink = if selected {
            ChromeInk::Identity
        } else {
            ChromeInk::MetadataValue
        };
        let mut label_style = chrome(theme, ink);
        if selected {
            label_style = label_style.add_modifier(Modifier::BOLD);
        }
        let option_label = if layout.strip.width < 56 {
            match index {
                0 => tr(startup.locale, MessageId::LaunchMenuWorktreeCompact),
                1 => tr(startup.locale, MessageId::LaunchMenuChatCompact),
                _ => Cow::Borrowed(option.label),
            }
        } else {
            Cow::Borrowed(option.label)
        };
        let label = format!("{} {option_label}", startup.sym(option.icon));
        let budget = if layout.strip.width < 56 {
            usize::from(column_w)
        } else {
            usize::from(column_w.saturating_sub(1))
        };
        let label = truncate_to_width(&label, budget);
        set_span(
            buf,
            x,
            layout.strip.y + layout.strip_content_start,
            &Span::styled(label, label_style),
        );
        set_span(
            buf,
            x,
            layout.strip.y + layout.strip_content_start + 1,
            &Span::styled(option.key, chrome(theme, ChromeInk::MetadataHint)),
        );
    }

    // The docked pre-session composer (§5b) is the same rounded Tideline
    // shell used by the work surface whenever the full four-row dock fits.
    // Its shell owns both the visible `[↑]` affordance and the matching
    // geometry; the launch renderer owns only localized input/hint content.
    // Tiny terminals retain the compact strip rather than drawing fake
    // corners with no interior cells.
    let enclosed = startup.composer.enclosed
        && layout.dock.height >= crate::tui::composer_chrome::TIDELINE_COMPOSER_HEIGHT
        && layout.dock.width >= 6;
    let composer_rows = if enclosed {
        launch_composer_rows(stage)
    } else {
        launch_compact_composer_rows(stage)
    };
    if let Some((input_row, hint_row)) = composer_rows {
        render_launch_composer(
            stage,
            buf,
            theme,
            &startup.composer,
            input_row,
            hint_row,
            enclosed.then_some(layout.dock),
            startup.status_line.as_deref(),
            startup.ascii_safe,
        );
        if !enclosed && let Some(line) = startup.status_line.as_deref() {
            let y = if layout.dock.height == 1 {
                input_row
            } else {
                layout
                    .dock
                    .bottom()
                    .saturating_sub(1)
                    .saturating_sub(stage.y)
            };
            set_span(
                buf,
                stage.x + 2,
                stage.y + y,
                &Span::styled(
                    truncate_to_width(line, usize::from(stage.width.saturating_sub(4))),
                    chrome(theme, ChromeInk::Metadata),
                ),
            );
        }
    }
}

/// Recorded interactive hitboxes for the startup stage (spec §6): each quick
/// action row and option-strip tile, plus the docked composer's focus and send
/// targets. The hero copy is deliberately non-interactive and has no hitbox.
#[derive(Debug, Clone, Default)]
pub struct TidelineStartupHitboxes {
    pub actions: Vec<Rect>,
    pub options: Vec<Rect>,
    /// The docked composer focus surface (click focuses, exactly like Tab).
    pub composer: Option<Rect>,
    /// The actual text-input row; completion menus anchor directly above it.
    pub input: Option<Rect>,
    /// The send glyph inside the composer row (click submits).
    pub send: Option<Rect>,
}

/// Compute the startup hitboxes for one render area. Pure geometry through
/// the same `startup_layout` the renderer uses, so rects match painted
/// cells wherever both run on the same stage.
#[must_use]
pub fn tideline_startup_hitboxes(stage: Rect) -> TidelineStartupHitboxes {
    tideline_startup_hitboxes_with_composer(stage, true)
}

/// Same geometry as [`tideline_startup_hitboxes`], respecting the explicit
/// compact-composer preference used by the current `App` projection.
#[must_use]
pub fn tideline_startup_hitboxes_with_composer(
    stage: Rect,
    enclosed: bool,
) -> TidelineStartupHitboxes {
    let mut out = TidelineStartupHitboxes::default();
    if stage.width < 8 || stage.height < 5 {
        return out;
    }
    let layout = startup_layout(stage);

    out.actions = (0..3)
        .map(|index| Rect {
            x: layout.quick.x + 2,
            y: layout.quick.y + layout.quick_rows_start + index,
            width: layout.quick.width.saturating_sub(4),
            height: 1,
        })
        .collect();
    let columns = layout.strip_columns;
    let column_w = layout.strip.width / columns.max(1);
    out.options = (0..columns)
        .map(|index| Rect {
            x: layout.strip.x + index * column_w,
            y: layout.strip.y + layout.strip_content_start,
            width: column_w,
            height: 2,
        })
        .collect();
    // The four-row dock reuses the exact rounded shell geometry, including
    // the visible three-cell `[↑]` submit rect. Compact terminals preserve
    // the older one-line target because they cannot host an enclosed shell.
    if layout.dock.height >= 1 {
        let use_enclosed = enclosed
            && layout.dock.height >= crate::tui::composer_chrome::TIDELINE_COMPOSER_HEIGHT
            && layout.dock.width >= 6;
        if use_enclosed {
            let geometry = crate::tui::composer_chrome::tideline_composer_geometry(layout.dock);
            let hitboxes = crate::tui::composer_chrome::tideline_composer_hitboxes(layout.dock);
            out.composer = Some(hitboxes.border);
            out.input = Some(Rect {
                x: geometry.content.x,
                y: geometry.content.y,
                width: geometry.content.width,
                height: 1,
            });
            out.send = Some(hitboxes.submit);
        } else {
            let input = Rect {
                x: stage.x.saturating_add(2),
                y: layout.dock.y,
                width: stage.width.saturating_sub(4),
                height: 1,
            };
            out.composer = Some(input);
            out.input = Some(input);
            out.send = Some(Rect {
                x: stage.x.saturating_add(stage.width.saturating_sub(4)),
                y: layout.dock.y,
                width: 2.min(stage.width),
                height: 1,
            });
        }
    }
    out
}

/// Mouse dispatch intent for one option-strip tile (spec §6: keyboard and
/// mouse parity). Every tile dispatches through the launch table — the
/// same `handle_launch_key` path its printed key takes; `launch_row` is
/// the table row the tile owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchOptionAction {
    Worktree,
    Chat,
    Theme,
    Help,
}

impl LaunchOptionAction {
    /// The tile order [`TidelineOption::approved_set`] paints.
    const STRIP: [Self; 4] = [Self::Worktree, Self::Chat, Self::Theme, Self::Help];

    /// The launch-table row this tile dispatches through (the same row its
    /// printed direct key selects in `handle_launch_key`).
    #[must_use]
    pub fn launch_row(self) -> usize {
        match self {
            Self::Worktree => 3,
            Self::Chat => 4,
            Self::Theme => 5,
            Self::Help => 6,
        }
    }
}

/// The startup stage's visible rows as launch-table indices: the three
/// QUICK ACTIONS (New session, Chat only, Resume last) are table rows
/// `[2, 4, 1]`; the option tiles (worktree, chat, theme, help) are rows
/// `[3, 4, 5, 6]`. Chat (4) shows focus on its quick-action row — the
/// richer affordance — and its tile stays plain. Connect (row 0) has no
/// stage home: its `P` direct key and the topbar's Model segment are its
/// routes, so focus resting there shows no marker (the input model is
/// main's exactly; the stage only projects it).
const QUICK_ACTION_ROWS: [usize; 3] = [2, 4, 1];
const OPTION_TILE_ROWS: [usize; 4] = [3, 4, 5, 6];

/// Project live `App` state onto the startup stage's inputs (spec §5a data
/// sources: `LaunchState.workspace_session_count`, provider onboarding, the
/// launch selection, and the previous frame's row hitboxes for hover — the
/// same one-frame-lag registry the topbar uses).
#[must_use]
pub fn tideline_startup_from_app(app: &App) -> TidelineStartup<'_> {
    let ascii_safe = crate::tui::color_compat::ascii_safe_enabled();
    // Hover resolves through the seven-slot row registry: the slot under
    // the mouse is a table row; its quick-action position (if any) is the
    // hovered row.
    let hovered_action = app.last_mouse_pos.and_then(|(mx, my)| {
        let slot = app
            .launch
            .row_areas
            .iter()
            .position(|area| area.x <= mx && mx < area.right() && area.y == my)?;
        QUICK_ACTION_ROWS.iter().position(|row| *row == slot)
    });
    let selected = app.launch.selected;
    let mut startup = TidelineStartup::new(
        &app.ui_theme,
        app.launch.workspace_session_count,
        !app.onboarding_needs_api_key,
    )
    .locale(app.ui_locale)
    .ascii_safe(ascii_safe)
    .composer(LaunchComposerDisplay::from_app(app))
    .status_line(launch_status_line(app, ascii_safe))
    // The app opens on this screen, so the ambient clock is already
    // launch-relative. Reduced motion asks for the settled mark, which is the
    // same drawing at its endpoint.
    .surface_progress(if app.motion_policy().allows_decorative() {
        crate::tui::mark::surface_progress(app.ambient_clock_ms, MARK_SURFACE_MS)
    } else {
        1.0
    });
    // Keyboard parity with the launch table: a quick action holds the
    // marker when the selected table row is one of QUICK_ACTION_ROWS; a
    // tile holds it when the row is that tile's; row 0 (Connect) rests
    // nowhere visible by design.
    startup.selected_action = QUICK_ACTION_ROWS.iter().position(|row| *row == selected);
    startup.selected_option = if startup.selected_action.is_some() {
        None
    } else {
        OPTION_TILE_ROWS.iter().position(|row| *row == selected)
    };
    startup.hovered_action = hovered_action;
    startup
}

/// The launch surface's transient line: the worktree-name prompt while the
/// name is being typed, else the most recent launch status message.
fn launch_status_line(app: &App, ascii_safe: bool) -> Option<String> {
    if let Some(input) = app.launch.worktree_input.as_deref() {
        let caret = if app.low_motion || ascii_safe {
            "_"
        } else {
            "▌"
        };
        Some(format!(
            "{}  {}{caret}",
            tr(app.ui_locale, MessageId::LaunchWorktreeNameLabel),
            input
        ))
    } else {
        app.launch.status.as_deref().map(str::to_string)
    }
}

/// Store the startup stage's clickable rects into the launch state — the
/// role #5698's `record_launch_hitboxes` owned, re-anchored to the stage.
/// Call after the stage is painted, with the hitboxes computed for the
/// same stage rect. `row_areas` is the launch table's seven slots (main's
/// meaning: index == the table row, so `mouse_ui`'s click path — set the
/// row, Enter — dispatches unchanged); the three quick-action rects land
/// at their `QUICK_ACTION_ROWS` slots and unreachable slots hold a
/// zero-size rect that never hit-tests. The option tiles land in the
/// typed `option_areas` registry, and the docked composer's input and
/// send rects in `composer_area`/`send_area` (main's fields, main's
/// shapes).
pub fn apply_launch_hitboxes(
    hitboxes: &TidelineStartupHitboxes,
    launch: &mut crate::tui::app::LaunchState,
) {
    let mut rows = vec![Rect::default(); LAUNCH_ROWS.len()];
    for (slot, area) in QUICK_ACTION_ROWS.iter().zip(hitboxes.actions.iter()) {
        if let Some(slot) = rows.get_mut(*slot) {
            *slot = *area;
        }
    }
    launch.row_areas = rows;
    launch.option_areas = hitboxes
        .options
        .iter()
        .enumerate()
        .filter_map(|(index, area)| {
            LaunchOptionAction::STRIP
                .get(index)
                .map(|action| (*action, *area))
        })
        .collect();
    launch.composer_area = hitboxes.composer;
    launch.send_area = hitboxes.send;
}

#[cfg(test)]
mod tideline_tests;
