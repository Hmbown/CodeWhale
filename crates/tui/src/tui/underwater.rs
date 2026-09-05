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
    ocean::COMPLETION_BREATH_MS,
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

/// What one launch key produces. The composer holds focus and takes every
/// ordinary key, so the only launch-owned input is F1 help; the card's
/// rows are driven by Up/Down + Enter (and the mouse) through
/// [`run_launch_card_row`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchAction {
    None,
    /// The prominent new-session entry: begin a fresh session in the
    /// current workspace.
    NewSession,
    /// Resume one recent-work row by session id.
    ResumeSession(String),
    /// The see-all overflow: open the full session picker.
    BrowseSessions,
    Help,
    /// Submit the composed pre-session message: begin the launch session,
    /// then hand the text to the normal composer dispatch path.
    SendComposer,
}

/// Translate a launch key into one product action. Reached only through
/// [`LaunchComposerKey::MenuChord`]; every other key belongs to the
/// composer authority.
pub fn handle_launch_key(
    _launch: &mut crate::tui::app::LaunchState,
    key: KeyEvent,
    _locale: Locale,
) -> LaunchAction {
    match key.code {
        KeyCode::F(1) => LaunchAction::Help,
        _ => LaunchAction::None,
    }
}

/// One interactive row on the startup card: the prominent new-session
/// entry, one recent-work row, or the see-all overflow. Labels are
/// localized; `detail` is right-aligned metadata (a recent row's age).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCardRow {
    pub id: crate::tui::app::LaunchRowId,
    pub label: String,
    pub detail: String,
    /// The new-session entry paints prominent (bold accent) when it is
    /// neither keyboard-selected nor hovered.
    pub prominent: bool,
}

/// A recent session projected for the card: the display title plus its
/// right-aligned detail line. Preformatted by the caller so the renderer
/// stays deterministic for golden buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRecentEntry {
    pub id: String,
    pub title: String,
    pub detail: String,
}

/// The card's rows in paint/click/keyboard order: the prominent
/// new-session entry first, then recent work, then the see-all overflow
/// when more sessions sit behind the inline list. The single ordering
/// keyboard, mouse, and paint share.
#[must_use]
pub fn launch_card_rows(
    locale: Locale,
    recent: &[LaunchRecentEntry],
    has_more: bool,
) -> Vec<LaunchCardRow> {
    let mut rows = Vec::with_capacity(recent.len() + 2);
    rows.push(LaunchCardRow {
        id: crate::tui::app::LaunchRowId::NewSession,
        label: tr(locale, MessageId::LaunchNewSession).into_owned(),
        detail: String::new(),
        prominent: true,
    });
    rows.extend(recent.iter().map(|entry| LaunchCardRow {
        id: crate::tui::app::LaunchRowId::Recent(entry.id.clone()),
        label: entry.title.clone(),
        detail: entry.detail.clone(),
        prominent: false,
    }));
    if has_more {
        rows.push(LaunchCardRow {
            id: crate::tui::app::LaunchRowId::SeeAll,
            label: tr(locale, MessageId::LaunchSeeAllSessions).into_owned(),
            detail: String::new(),
            prominent: false,
        });
    }
    rows
}

/// Project the launch state's loaded recent-work list into card entries:
/// display titles with right-aligned relative ages, like the resume
/// picker. Pure projection of loaded state — no disk reads.
fn launch_recent_entries(app: &App) -> (Vec<LaunchRecentEntry>, bool) {
    let recent = app
        .launch
        .recent
        .iter()
        .map(|session| {
            let raw = crate::session_manager::extract_title(&session.title);
            let title = if raw == "Session" || raw.trim().is_empty() {
                crate::session_manager::truncate_id(&session.id).to_string()
            } else {
                raw.to_string()
            };
            let age = crate::tui::session_picker::format_relative_time(
                &session.updated_at,
                app.ui_locale,
            );
            let count = tr(app.ui_locale, MessageId::SessionsMessageCountCompact)
                .replace("{count}", &session.message_count.to_string());
            LaunchRecentEntry {
                id: session.id.clone(),
                title,
                detail: format!("{age} · {count}"),
            }
        })
        .collect::<Vec<_>>();
    let has_more = app.launch.total_workspace_sessions > recent.len();
    (recent, has_more)
}

/// The card's rows for live `App` state, for keyboard navigation and
/// Enter — the same [`launch_card_rows`] order paint and hitboxes share.
#[must_use]
pub fn launch_rows_for_app(app: &App) -> Vec<LaunchCardRow> {
    let (recent, has_more) = launch_recent_entries(app);
    launch_card_rows(app.ui_locale, &recent, has_more)
}

/// The click twin of [`run_launch_card_row`]: one card row id runs the
/// same action the keyboard's Enter runs, so mouse and keyboard share one
/// contract.
#[must_use]
pub fn launch_row_click_action(id: &crate::tui::app::LaunchRowId) -> LaunchAction {
    match id {
        crate::tui::app::LaunchRowId::NewSession => LaunchAction::NewSession,
        crate::tui::app::LaunchRowId::Recent(session_id) => {
            LaunchAction::ResumeSession(session_id.clone())
        }
        crate::tui::app::LaunchRowId::SeeAll => LaunchAction::BrowseSessions,
    }
}

/// Run the card's highlighted row. Enter on the card is the list's runner;
/// an untouched list runs nothing.
pub fn run_launch_card_row(rows: &[LaunchCardRow], menu_selected: Option<usize>) -> LaunchAction {
    let Some(selected) = menu_selected else {
        return LaunchAction::None;
    };
    match rows.get(selected) {
        None => LaunchAction::None,
        Some(row) => match &row.id {
            crate::tui::app::LaunchRowId::NewSession => LaunchAction::NewSession,
            crate::tui::app::LaunchRowId::Recent(id) => LaunchAction::ResumeSession(id.clone()),
            crate::tui::app::LaunchRowId::SeeAll => LaunchAction::BrowseSessions,
        },
    }
}

/// What the pre-session composer layer decided about one key.
///
/// This is only an admission guard, never an input implementation: the
/// startup composer is the session's own [`crate::tui::app::ComposerState`],
/// and every editing key is answered by the conversation composer match in
/// the event loop — the single composer input authority — exactly as it
/// would be in a live session. Word motion, selection, completion menus,
/// attachments, history, paste bursts, and vim behaviour therefore cannot
/// drift from the shell. Only three things are launch-specific here: an
/// empty Enter, F1 help, and submitting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchComposerKey {
    /// The key is fully consumed and does nothing more (Enter on an empty
    /// composer with no menu entry highlighted: there is no row to run and
    /// nothing to send; Esc clearing the menu highlight or bringing the
    /// card back).
    Consumed,
    /// Submit the composed message through the normal dispatch path.
    Submit,
    /// A completion-menu selection was applied (a slash or mention popup was
    /// open and Enter picked the highlighted entry); the key is consumed
    /// without submitting — the completed text stays in the composer.
    MenuSelect,
    /// The launch chord (F1 help): the same key is then handed to
    /// [`handle_launch_key`]. It deliberately wins over its composer
    /// meaning while the launch screen is up.
    MenuChord,
    /// Not launch-specific: the conversation composer match below owns the
    /// key. The event loop must not run [`handle_launch_key`] for it.
    ComposerAuthority,
    /// Move the launch card's row selection (Up/Down while the card is up).
    MenuNavigate(i32),
    /// Run the card's highlighted row (Enter while the card is up, the
    /// composer is empty, and the user has arrowed onto a row).
    MenuRun,
}

/// Admit one key for the pre-session composer.
///
/// Editing keys are never handled here — they fall through to the
/// conversation composer match so there is exactly one composer input
/// system. Only F1 help stays launch-owned via
/// [`LaunchComposerKey::MenuChord`].
pub fn handle_launch_composer_key(app: &mut App, key: KeyEvent) -> LaunchComposerKey {
    let multiline = app.composer_multiline_mode;
    let card_up = app.launch.dissolve_started_ms.is_none();
    match key.code {
        KeyCode::Enter
            if crate::tui::composer_ui::composer_submit_chord(key, multiline).is_some() =>
        {
            // #573 parity with the session composer's Enter arm: when a
            // completion popup is matching (e.g. `/mo` → `/model`), Enter
            // applies the highlighted entry instead of sending the literal
            // prefix. A mention completion amends the composed text and is
            // consumed; a slash completion completes the command and falls
            // through to Submit so the launch dispatch path executes it.
            let mention_entries = crate::tui::file_mention::visible_mention_menu_entries(app, 1);
            if !mention_entries.is_empty()
                && crate::tui::file_mention::apply_mention_menu_selection(app, &mention_entries)
            {
                return LaunchComposerKey::MenuSelect;
            }
            let slash_entries = crate::tui::slash_menu::visible_slash_menu_entries(app, 1);
            if !slash_entries.is_empty() {
                crate::tui::slash_menu::apply_slash_menu_selection(app, &slash_entries, false);
                app.close_slash_menu();
            }
            if app.input.trim().is_empty() {
                if card_up && app.launch.menu_selected.is_some() {
                    // The card owns Enter only once the user has arrowed
                    // onto a row; an untouched list runs nothing.
                    return LaunchComposerKey::MenuRun;
                }
                LaunchComposerKey::Consumed
            } else {
                app.launch.dissolve_card(app.ambient_clock_ms);
                LaunchComposerKey::Submit
            }
        }
        KeyCode::Up if card_up => LaunchComposerKey::MenuNavigate(-1),
        KeyCode::Down if card_up => LaunchComposerKey::MenuNavigate(1),
        // Esc walks back one step: a highlighted row is unhighlighted;
        // an empty composer with the card gone brings the card back. A draft
        // in the composer keeps Esc's composer meaning.
        KeyCode::Esc if card_up && app.launch.menu_selected.is_some() => {
            app.launch.menu_selected = None;
            LaunchComposerKey::Consumed
        }
        KeyCode::Esc if !card_up && app.input.is_empty() => {
            app.launch.restore_card();
            LaunchComposerKey::Consumed
        }
        KeyCode::F(1) => LaunchComposerKey::MenuChord,
        // Every other key — text, caret motion, word motion, selection,
        // newline chords, Home/End, kill/chord editing, vim motions, Esc,
        // Tab, history — is answered by the conversation composer authority.
        _ => {
            // Typing goes straight to the composer, and the first keystroke
            // dissolves the card (founder decision, 2026-09-02).
            if card_up
                && matches!(key.code, KeyCode::Char(_))
                && !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
            {
                app.launch.dissolve_card(app.ambient_clock_ms);
            }
            LaunchComposerKey::ComposerAuthority
        }
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
        AppMode::Agent => ChromeInk::PolicyAct,
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
            LiveActivityKind::UsingSubagents => "fleet underway…",
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
        AppMode::Agent => tr(locale, MessageId::ChipModeAct),
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
/// `files: workspace (unenforced)` and the Full-Access-but-confined case land
/// as warnings when they do appear.
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
        // `DangerFullAccess` only ever arises from the Bypass posture
        // (`sandbox_policy_for_turn`), whose permission chip already reads
        // "Full Access" two words to the left. The name is the disclosure;
        // restating it as `files: full disk` spent columns saying it twice.
        // The scope chip speaks in this posture only when the scope is
        // *narrower* than the name implies (the WorkspaceWrite arm below).
        crate::sandbox::SandboxPolicy::DangerFullAccess => None,
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
    /// The empty composer's placeholder.
    pub placeholder: std::borrow::Cow<'a, str>,
    /// The composer's hint line (Enter / Shift+Enter / Esc). The stage's
    /// transient status line paints over it while the worktree prompt has
    /// the keyboard.
    pub hint: std::borrow::Cow<'a, str>,
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
            hint: Cow::Borrowed(""),
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
            hint: tr(app.ui_locale, MessageId::LaunchComposerHint),
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
#[allow(clippy::too_many_arguments)] // pre-existing baseline signature; FEAT-022 gate repair
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
    bottom_rule: Option<&str>,
) {
    let focused = display.focused;
    if let Some(panel_area) = panel_area {
        crate::tui::composer_chrome::render_tideline_composer_shell(
            panel_area, buf, theme, focused, ascii_safe,
        );
        // The launch rule: `model (effort) · permission` trailing right on
        // the bottom border — the route's one reading while the card is up.
        if let Some(rule) = bottom_rule
            && panel_area.height >= 4
        {
            let rule = truncate_to_width(rule, usize::from(panel_area.width.saturating_sub(4)));
            let rule_w = rule.width() as u16;
            let rule_x = panel_area
                .right()
                .saturating_sub(1)
                .saturating_sub(rule_w)
                .max(panel_area.x + 1);
            set_span(
                buf,
                rule_x,
                panel_area.y + panel_area.height - 1,
                &Span::styled(rule, chrome(theme, ChromeInk::Metadata)),
            );
        }
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

        let hint = status_line
            .map(Cow::Borrowed)
            .unwrap_or_else(|| display.hint.clone());
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
        let hint = display.hint.clone();
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
    let brand = "codewhale";
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
        LaunchAction, LaunchRecentEntry, handle_launch_key, launch_card_rows,
        launch_row_click_action, run_launch_card_row,
    };
    use crate::localization::Locale;
    use crate::tui::app::{LaunchRowId, LaunchState};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn launch_state() -> LaunchState {
        LaunchState {
            visible: true,
            status: None,
            workspace: std::env::temp_dir(),
            recent: Vec::new(),
            total_workspace_sessions: 0,
            composer_focus: true,
            composer_area: None,
            send_area: None,
            row_hitboxes: Vec::new(),
            hovered_row: None,
            menu_selected: None,
            dissolve_started_ms: None,
            claude_code_detected: false,
            sixel_cell_px: None,
            sixel_terminal_bg: None,
            sixel_mark_area: None,
            sixel_emitted: None,
        }
    }

    fn recent_entry(id: &str) -> LaunchRecentEntry {
        LaunchRecentEntry {
            id: id.to_string(),
            title: format!("title {id}"),
            detail: "2h ago · 4 msgs".to_string(),
        }
    }

    #[test]
    fn only_f1_survives_as_a_launch_key() {
        // The old ctrl+n/r/l/q menu chords are gone: those keys belong to
        // the composer authority now, so the launch key handler yields
        // nothing for them.
        let key = |code, mods| KeyEvent::new(code, mods);
        let ctrl = KeyModifiers::CONTROL;
        let none = KeyModifiers::NONE;
        let mut launch = launch_state();
        assert_eq!(
            handle_launch_key(&mut launch, key(KeyCode::F(1), none), Locale::En),
            LaunchAction::Help
        );
        assert!(launch.composer_focus, "F1 leaves the composer focused");
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('r'),
            KeyCode::Char('l'),
            KeyCode::Char('q'),
            KeyCode::Char('p'),
            KeyCode::Char('w'),
            KeyCode::Enter,
            KeyCode::Down,
        ] {
            for mods in [none, ctrl] {
                let mut launch = launch_state();
                assert_eq!(
                    handle_launch_key(&mut launch, key(code, mods), Locale::En),
                    LaunchAction::None,
                    "{code:?} with {mods:?} is not a launch action"
                );
            }
        }
    }

    #[test]
    fn card_rows_run_new_resume_and_see_all() {
        let rows = launch_card_rows(
            Locale::En,
            &[recent_entry("abc"), recent_entry("def")],
            true,
        );
        // New session, two recents, then the see-all overflow.
        assert_eq!(rows.len(), 4);
        assert!(rows[0].prominent);
        assert_eq!(run_launch_card_row(&rows, None), LaunchAction::None);
        assert_eq!(
            run_launch_card_row(&rows, Some(0)),
            LaunchAction::NewSession
        );
        assert_eq!(
            run_launch_card_row(&rows, Some(1)),
            LaunchAction::ResumeSession("abc".to_string())
        );
        assert_eq!(
            run_launch_card_row(&rows, Some(2)),
            LaunchAction::ResumeSession("def".to_string())
        );
        assert_eq!(
            run_launch_card_row(&rows, Some(3)),
            LaunchAction::BrowseSessions
        );
        assert_eq!(run_launch_card_row(&rows, Some(99)), LaunchAction::None);
        // Clicks run the same actions as the keyboard's Enter.
        assert_eq!(
            launch_row_click_action(&LaunchRowId::NewSession),
            LaunchAction::NewSession
        );
        assert_eq!(
            launch_row_click_action(&LaunchRowId::Recent("abc".to_string())),
            LaunchAction::ResumeSession("abc".to_string())
        );
        assert_eq!(
            launch_row_click_action(&LaunchRowId::SeeAll),
            LaunchAction::BrowseSessions
        );
    }

    #[test]
    fn card_rows_omit_the_overflow_when_nothing_sits_behind() {
        let rows = launch_card_rows(Locale::En, &[], false);
        assert_eq!(rows.len(), 1, "only the new-session entry");
        assert!(rows[0].prominent);
        assert_eq!(
            run_launch_card_row(&rows, Some(0)),
            LaunchAction::NewSession
        );
    }
}

#[cfg(test)]
mod launch_composer_tests {
    use super::{
        LaunchAction, LaunchComposerKey, apply_launch_hitboxes, handle_launch_composer_key,
        handle_launch_key, launch_composer_rows, launch_rows_for_app,
        render_launch_completion_popup, render_tideline_startup, run_launch_card_row,
        tideline_startup_from_app, tideline_startup_hitboxes,
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
    /// projected startup widget (header, state line, docked composer), with
    /// the hitboxes applied as the frame applies them.
    fn render(app: &App, width: u16, height: u16) -> (Buffer, Rect) {
        let area = stage_for(width, height);
        let mut buf = Buffer::empty(area);
        let startup = tideline_startup_from_app(app);
        render_tideline_startup(area, &mut buf, &startup);
        let mut hitboxes = tideline_startup_hitboxes(area);
        hitboxes.rows = super::tideline_startup_row_hitboxes(area, &startup);
        let mut launch = app.launch.clone();
        apply_launch_hitboxes(&hitboxes, &mut launch);
        (buf, area)
    }

    fn recent_fixture(id: &str, title: &str) -> super::LaunchRecentEntry {
        super::LaunchRecentEntry {
            id: id.to_string(),
            title: title.to_string(),
            detail: "2h ago · 4 msgs".to_string(),
        }
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

    /// Cell columns `from..to` of row `y` (area-relative, byte-safe against
    /// wide glyphs).
    fn row_cells(buf: &Buffer, area: Rect, y: u16, from: u16, to: u16) -> String {
        (from..to.min(area.x + area.width))
            .map(|x| buf[(x, area.y + y)].symbol().to_string())
            .collect()
    }

    #[test]
    fn card_lists_new_session_over_recent_work() {
        let app = launch_app();
        let area = stage_for(100, 30);
        let mut startup = tideline_startup_from_app(&app);
        startup.recent = vec![
            recent_fixture("abc", "Fix login flow"),
            recent_fixture("def", "Plan export"),
        ];
        startup.has_more_recent = true;
        let mut buf = Buffer::empty(area);
        render_tideline_startup(area, &mut buf, &startup);
        let text = (0..area.height)
            .map(|y| row_text(&buf, area, y))
            .collect::<Vec<_>>()
            .join("\n");
        for fact in [
            "codewhale",
            "New session",
            "Recent",
            "Fix login flow",
            "Plan export",
            "2h ago",
            "See all sessions",
        ] {
            assert!(text.contains(fact), "missing {fact:?} in:\n{text}");
        }
        // The prominent entry leads; the old menu is gone entirely.
        assert!(
            text.find("New session").unwrap() < text.find("Fix login flow").unwrap(),
            "new session leads the list:\n{text}"
        );
        for gone in [
            "New worktree",
            "Resume session",
            "Changelog",
            "Quit",
            "ctrl+n",
            "ctrl+r",
            "ctrl+l",
            "ctrl+q",
        ] {
            assert!(!text.contains(gone), "{gone:?} is back:\n{text}");
        }
    }

    #[test]
    fn empty_workspace_points_at_the_composer() {
        let app = launch_app();
        let area = stage_for(100, 30);
        let mut startup = tideline_startup_from_app(&app);
        startup.recent = Vec::new();
        startup.has_more_recent = false;
        let mut buf = Buffer::empty(area);
        render_tideline_startup(area, &mut buf, &startup);
        let text = (0..area.height)
            .map(|y| row_text(&buf, area, y))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("New session"), "the entry survives:\n{text}");
        assert!(
            text.contains("No recent sessions"),
            "empty workspaces say so:\n{text}"
        );
        assert!(
            !text.contains("See all sessions"),
            "no overflow without sessions:\n{text}"
        );
    }

    #[test]
    fn row_hitboxes_match_painted_cells_and_hover_highlights() {
        use crate::tui::app::LaunchRowId;
        let app = launch_app();
        let area = stage_for(100, 30);
        let mut startup = tideline_startup_from_app(&app);
        startup.recent = vec![recent_fixture("abc", "Fix login flow")];
        startup.has_more_recent = true;
        let mut buf = Buffer::empty(area);
        render_tideline_startup(area, &mut buf, &startup);
        let rows = super::tideline_startup_row_hitboxes(area, &startup);
        assert_eq!(
            rows.iter().map(|(id, _)| id.clone()).collect::<Vec<_>>(),
            vec![
                LaunchRowId::NewSession,
                LaunchRowId::Recent("abc".to_string()),
                LaunchRowId::SeeAll,
            ]
        );
        for (_, rect) in &rows {
            let painted: String = (rect.x..rect.x + rect.width)
                .map(|x| buf[(x, rect.y)].symbol().to_string())
                .collect();
            assert!(!painted.trim().is_empty(), "row hitbox covers empty cells");
        }
        // Hover paints the shared selection band on exactly the hovered
        // row — the visible response every clickable element owes.
        startup.hovered = Some(1);
        let mut buf = Buffer::empty(area);
        render_tideline_startup(area, &mut buf, &startup);
        for (index, (_, rect)) in rows.iter().enumerate() {
            let banded = (rect.x..rect.x + rect.width)
                .filter(|x| buf[(*x, rect.y)].bg == crate::palette::SELECTION_BG)
                .count();
            if index == 1 {
                assert!(banded > 0, "the hovered row carries the selection band");
            } else {
                assert_eq!(banded, 0, "only the hovered row highlights");
            }
        }
        // Keyboard selection paints the same band.
        startup.hovered = None;
        startup.menu_selected = Some(0);
        let mut buf = Buffer::empty(area);
        render_tideline_startup(area, &mut buf, &startup);
        let (_, first) = &rows[0];
        assert!(
            (first.x..first.x + first.width)
                .any(|x| buf[(x, first.y)].bg == crate::palette::SELECTION_BG),
            "keyboard selection paints the same band as hover"
        );
    }

    #[test]
    fn composer_docks_focused_at_every_supported_size() {
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
                input_row.contains('▌'),
                "{width}x{height}: the composer is focused from first paint: {input_row:?}"
            );
            assert!(
                !row_text(&buf, area, hint_y).contains("Tab to type"),
                "{width}x{height}: nothing to tab into; the composer already has focus"
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
    fn the_top_line_never_collides_with_the_composer_dock() {
        // The top line (`⑂ branch  path`) owns row 0; the dock owns the
        // bottom rows. At the 40x12 floor the stage is ten rows: the top
        // line, the centred card, and the dock's four.
        for (width, height) in LAUNCH_SIZES {
            let app = launch_app();
            let (buf, area) = render(&app, width, height);
            let (input_y, _) = launch_composer_rows(area).unwrap();
            assert!(input_y >= 2, "{width}x{height}: dock below the top line");
            assert!(
                !row_text(&buf, area, 0).trim().is_empty(),
                "{width}x{height}: the top line paints the branch/path: {:?}",
                row_text(&buf, area, 0)
            );
        }
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
        assert!(app.launch.composer_focus, "focused from first paint");

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

        // The old startup shortcut letters are ordinary text now…
        type_char(&mut app, 'p');
        assert_eq!(app.input, "hp");

        // …and word motion is composer-owned too: Alt+B moves a whole word
        // back through the exact shared helper the conversation composer
        // uses.
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

        // Esc, Tab and the arrows have no launch meaning: nothing to blur to.
        for code in [KeyCode::Esc, KeyCode::Tab, KeyCode::Up, KeyCode::Down] {
            assert_eq!(
                handle_launch_composer_key(&mut app, KeyEvent::new(code, KeyModifiers::NONE)),
                LaunchComposerKey::ComposerAuthority,
                "{code:?} belongs to the composer"
            );
            assert!(app.launch.composer_focus);
        }

        // …while F1 help stays launch-owned.
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
            LaunchComposerKey::MenuChord
        );
        assert!(app.launch.composer_focus);
        assert_eq!(
            handle_launch_key(
                &mut app.launch,
                KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE),
                Locale::En,
            ),
            LaunchAction::Help
        );
        // The old ctrl+n/r/l/q menu chords are composer keys now: the
        // admission guard omits them to the composer authority.
        for code in [
            KeyCode::Char('n'),
            KeyCode::Char('r'),
            KeyCode::Char('l'),
            KeyCode::Char('q'),
        ] {
            assert_eq!(
                handle_launch_composer_key(&mut app, KeyEvent::new(code, KeyModifiers::CONTROL)),
                LaunchComposerKey::ComposerAuthority,
                "{code:?} belongs to the composer now"
            );
        }
    }

    #[test]
    fn composer_enter_probe_mirrors_the_real_enter_without_mutating() {
        let mut app = launch_app();
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

        // Enter on an empty composer with an untouched list runs nothing:
        // no row is pre-selected, so a reflexive Enter at launch cannot
        // start or resume a session by accident (founder live-test,
        // 2026-09-02).
        let mut empty = launch_app();
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        assert_eq!(empty.launch.menu_selected, None);
        assert_eq!(
            handle_launch_composer_key(&mut empty, enter),
            LaunchComposerKey::Consumed
        );
        assert_eq!(
            run_launch_card_row(&launch_rows_for_app(&empty), empty.launch.menu_selected),
            LaunchAction::None
        );
        assert!(empty.launch.composer_focus);
        // Once the user has arrowed onto a row, Enter runs it: row 0 is
        // the prominent new-session entry.
        assert_eq!(
            handle_launch_composer_key(
                &mut empty,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)
            ),
            LaunchComposerKey::MenuNavigate(1)
        );
        empty.launch.menu_selected = Some(0);
        assert_eq!(
            handle_launch_composer_key(&mut empty, enter),
            LaunchComposerKey::MenuRun
        );
        assert_eq!(
            run_launch_card_row(&launch_rows_for_app(&empty), empty.launch.menu_selected),
            LaunchAction::NewSession
        );
        // Esc unhighlights the list instead of reaching the composer.
        assert_eq!(
            handle_launch_composer_key(&mut empty, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            LaunchComposerKey::Consumed
        );
        assert_eq!(empty.launch.menu_selected, None);
        // Once the card has dissolved, empty-composer Enter is consumed:
        // there is no row to run and nothing to send.
        empty.launch.dissolve_started_ms = Some(0);
        assert_eq!(
            handle_launch_composer_key(&mut empty, enter),
            LaunchComposerKey::Consumed
        );
        // And Esc on the empty composer brings the card back.
        assert_eq!(
            handle_launch_composer_key(&mut empty, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            LaunchComposerKey::Consumed
        );
        assert_eq!(empty.launch.dissolve_started_ms, None);
    }

    #[test]
    fn highlighted_new_session_row_runs_a_fresh_session() {
        // Row 0 is always the prominent new-session entry.
        let app = launch_app();
        let rows = launch_rows_for_app(&app);
        assert!(!rows.is_empty(), "the card always lists a first row");
        assert_eq!(
            run_launch_card_row(&rows, Some(0)),
            LaunchAction::NewSession
        );
        // Esc with a highlighted row unhighlights instead of dissolving:
        // the card is still up.
        let mut app = app;
        app.launch.menu_selected = Some(0);
        assert_eq!(
            handle_launch_composer_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            LaunchComposerKey::Consumed
        );
        assert_eq!(app.launch.menu_selected, None);
        assert!(
            app.launch.dissolve_started_ms.is_none(),
            "the card is still up"
        );
    }

    #[test]
    fn shift_enter_keeps_a_real_newline_in_the_composer_state() {
        let mut app = launch_app();
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
    fn floor_keeps_a_usable_composer_row_and_the_next_tier_keeps_its_hint() {
        // The 40x12 floor's stage is 10 rows: the dock sheds to its input
        // row and the composer never disappears (the data — caret, draft —
        // survives; only the hint surface sheds, and it returns one tier up).
        let mut app = launch_app();
        let stage = stage_for(40, 12);
        let hitboxes = tideline_startup_hitboxes(stage);
        apply_launch_hitboxes(&hitboxes, &mut app.launch);
        assert!(app.launch.composer_area.is_some() && app.launch.send_area.is_some());
        let (buf, area) = render(&app, 40, 12);
        let (input_y, hint_y) = launch_composer_rows(stage).unwrap();
        let input_row = row_text(&buf, area, input_y);
        assert!(
            input_row.contains('❯') && input_row.contains('▌'),
            "focused floor composer keeps its anchors and caret: {input_row:?}"
        );
        assert!(hint_y <= area.height);

        // One tier up (a 22-row terminal, stage 20) the hint row explains
        // Enter/Esc — the focused composer's hint, since it is always focused.
        let (buf, area) = render(&app, 80, 22);
        let (_, hint_y) = launch_composer_rows(stage_for(80, 22)).unwrap();
        let hint_row = row_text(&buf, area, hint_y);
        assert!(
            hint_row.contains(
                &tr(Locale::En, MessageId::LaunchComposerHint)
                    .chars()
                    .take(20)
                    .collect::<String>()
            ),
            "focused dock must carry the composer hint: {hint_row:?}"
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
        assert!(line.starts_with("codewhale"), "{line:?}");
        assert!(line.contains("work"), "{line:?}");
        assert!(line.contains("ask"), "{line:?}");
    }

    #[test]
    fn full_access_is_the_disclosure_and_is_not_restated() {
        // Full disk access is stated once, by the permission chip's own
        // name. A second `files: full disk` chip beside it said the same
        // thing twice; the mode name stays prominent and does the work.
        let mut app = app();
        app.approval_mode = ApprovalMode::Bypass;
        app.configured_sandbox_mode = Some("danger-full-access".to_string());
        assert!(filesystem_scope_notice(&app).is_none());
        let line = header_line(&app, 120);
        assert!(!line.contains("files:"), "{line:?}");
        assert!(
            line.contains(&*super::tr(
                app.ui_locale,
                super::MessageId::ChipPermissionFullAccess
            )),
            "{line:?}"
        );
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
// Tideline startup stage — the launch header (shell design §2.0 item 2,
// founder direction 2026-09-02: "Claude Code's structure, not a centred hero
// with quick actions"). Top-left of the stage:
//
//   <mark>  Codewhale v0.9.12
//   <mark>  openrouter · deepseek-v4        (or `not connected`, gate ink)
//   <mark>  owner/repo · branch             (or the workspace path)
//
//   ⚠ no model connected · run /provider    (only while it is true)
//   ● 2 MCP servers connected · 1 needs sign-in · run /mcp   (only if true)
//
// then room, then the docked pre-session composer. Nothing else: no heading,
// no quick actions, no option strip, no wave rules. The stage is a pure,
// deterministic widget fed injected facts (`tideline_startup_from_app`
// projects `App`), proven against golden buffers `startup_{w}x{h}`.
// ---------------------------------------------------------------------------

use crate::palette::UiTheme;

/// How long the hero mark takes to surface, then it holds still forever.
const MARK_SURFACE_MS: u128 = 640;
/// Left margin before the mark and the header block.
const HEADER_MARGIN: u16 = 1;
/// One space between the mark and the header lines.
const HEADER_GUTTER: u16 = 1;
/// The header block: wordmark + version, route, workspace.
const HEADER_ROWS: u16 = 3;
/// Stages narrower than this paint the tiny mark so the header keeps columns.
const TINY_MARK_BELOW_WIDTH: u16 = 40;
/// Route facts are shed by `route_identity_fields` against this budget and
/// then truncated to the stage, so a long model name never clips a provider.
const ROUTE_BUDGET: usize = 60;

/// Which mark the stage paints. Decided by the caller from the terminal
/// (`kitty_graphics_supported`, `sixel_graphics_supported`,
/// `ascii_safe_enabled`), never in here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkTier {
    /// Kitty graphics placeholders over the transmitted PNG.
    Image,
    /// A blank block the event loop draws the sixel raster over after the
    /// frame. Same block size as [`MarkTier::Image`]; same PNG.
    Sixel,
    /// The braille rows.
    Braille,
    /// ASCII-safe: no mark, the wordmark line stands alone.
    None,
}

/// MCP facts: the working screen's `⋮ MCP n/m` chip and the card's
/// sign-in news. Only painted when there is something true to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct McpFacts {
    pub connected: usize,
    pub needs_sign_in: usize,
    /// Enabled servers total — the `m` in `n/m`.
    pub enabled: usize,
}

impl McpFacts {
    /// Count the servers the manager snapshot reports connected and those
    /// waiting on a sign-in. Disabled servers are nobody's business here.
    #[must_use]
    pub fn from_snapshot(snapshot: &crate::mcp::McpManagerSnapshot) -> Self {
        let enabled = snapshot.servers.iter().filter(|server| server.enabled);
        let (connected, needs_sign_in, total) = enabled.fold((0, 0, 0), |acc, server| {
            (
                acc.0 + usize::from(server.connected),
                acc.1 + usize::from(server.auth_required && !server.connected),
                acc.2 + 1,
            )
        });
        Self {
            connected,
            needs_sign_in,
            enabled: total,
        }
    }

    fn has_news(self) -> bool {
        self.connected > 0 || self.needs_sign_in > 0
    }
}

/// What the caller owes the startup stage. Everything injectable so renders
/// stay deterministic for golden buffers.
pub struct TidelineStartup<'a> {
    pub theme: &'a UiTheme,
    pub locale: Locale,
    /// The build version, painted dim after the wordmark.
    pub version: &'a str,
    /// The route line — the info line's own `route_identity_fields`, joined.
    /// `None` is "no model connected": line 2 says `not connected` and the
    /// state line says how to fix it.
    pub route: Option<String>,
    /// The workspace line: `owner/repo · branch` when the shell observes an
    /// origin slug, else the workspace path.
    pub workspace: String,
    pub mcp: Option<McpFacts>,
    /// The launch surface's one transient line — the worktree-name prompt or
    /// a launch status message — painted over the composer dock's last row.
    pub status_line: Option<String>,
    /// The docked pre-session composer's display projection.
    pub composer: LaunchComposerDisplay<'a>,
    /// ASCII-safe / NO_COLOR mode: every glyph through `ascii_fallback`.
    pub ascii_safe: bool,
    pub mark: MarkTier,
    /// How far the mark has surfaced, in `[0,1]`. Injected rather than read
    /// from a clock in here so golden buffers stay deterministic and so the
    /// reduced-motion path is the *same* drawing at its endpoint. Callers
    /// pass `1.0` for a settled mark.
    pub surface_progress: f32,
    /// How far the launch card has dissolved, `[0.0 intact ..= 1.0 gone]`.
    /// Injected for the same determinism as `surface_progress`.
    pub card_dissolve: f32,
    /// Recent work for the card's recent-work list, most recent first.
    pub recent: Vec<LaunchRecentEntry>,
    /// More workspace sessions sit behind `recent`: the card paints the
    /// see-all overflow row.
    pub has_more_recent: bool,
    /// The card row's highlighted entry, if the user has arrowed onto one.
    pub menu_selected: Option<usize>,
    /// The card row under the pointer, if any (index into the rows
    /// [`launch_card_rows`] yields for this stage).
    pub hovered: Option<usize>,
    /// The one migration notice above the composer, only when true.
    pub notice: Option<String>,
    /// `model (effort) · permission` — the composer bottom rule's trailing
    /// text while the card is up. The route's one launch reading.
    pub composer_rule: Option<String>,
    /// The branch for the top line, when the shell observes one.
    pub branch: Option<String>,
    /// Session-start hooks configured, for the working screen's receipt
    /// row. Zero paints the receipt without a hooks count.
    pub session_hooks: usize,
}

impl<'a> TidelineStartup<'a> {
    #[must_use]
    pub fn new(theme: &'a UiTheme, route: Option<String>, workspace: String) -> Self {
        Self {
            theme,
            locale: Locale::En,
            version: env!("CODEWHALE_BUILD_VERSION"),
            route,
            workspace,
            mcp: None,
            status_line: None,
            composer: LaunchComposerDisplay::default(),
            ascii_safe: false,
            mark: MarkTier::Braille,
            surface_progress: 1.0,
            card_dissolve: 0.0,
            recent: Vec::new(),
            has_more_recent: false,
            menu_selected: None,
            hovered: None,
            notice: None,
            composer_rule: None,
            branch: None,
            session_hooks: 0,
        }
    }

    /// Set the configured session-start hook count for the receipt row.
    #[must_use]
    pub fn session_hooks(mut self, hooks: usize) -> Self {
        self.session_hooks = hooks;
        self
    }

    /// Set the card's dissolve progress (`0.0` intact → `1.0` gone).
    #[must_use]
    pub fn card_dissolve(mut self, progress: f32) -> Self {
        self.card_dissolve = progress.clamp(0.0, 1.0);
        self
    }

    /// Set the card's recent-work list and whether more sessions sit
    /// behind it (the see-all overflow row).
    #[must_use]
    pub fn recent(mut self, recent: Vec<LaunchRecentEntry>, has_more: bool) -> Self {
        self.recent = recent;
        self.has_more_recent = has_more;
        self
    }

    /// Set the card row's highlighted entry.
    #[must_use]
    pub fn menu_selected(mut self, selected: Option<usize>) -> Self {
        self.menu_selected = selected;
        self
    }

    /// Set the card row under the pointer.
    #[must_use]
    pub fn hovered(mut self, hovered: Option<usize>) -> Self {
        self.hovered = hovered;
        self
    }

    /// Set the migration notice line above the composer.
    #[must_use]
    pub fn notice(mut self, notice: Option<String>) -> Self {
        self.notice = notice;
        self
    }

    /// Set the composer bottom rule's trailing text.
    #[must_use]
    pub fn composer_rule(mut self, rule: Option<String>) -> Self {
        self.composer_rule = rule;
        self
    }

    /// Set the observed git branch for the top line.
    #[must_use]
    pub fn branch(mut self, branch: Option<String>) -> Self {
        self.branch = branch;
        self
    }

    /// Set the mark's surface progress (`0.0` submerged → `1.0` settled).
    #[must_use]
    pub fn surface_progress(mut self, progress: f32) -> Self {
        self.surface_progress = progress.clamp(0.0, 1.0);
        self
    }

    #[must_use]
    pub fn mcp(mut self, mcp: Option<McpFacts>) -> Self {
        self.mcp = mcp;
        self
    }

    #[must_use]
    pub fn mark(mut self, mark: MarkTier) -> Self {
        self.mark = mark;
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
        if ascii_safe {
            self.mark = MarkTier::None;
        }
        self
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

    /// The card's announcement line, if there is anything true to say:
    /// the missing-model warning, else MCP news.
    fn state_line(&self) -> Option<(String, ChromeInk)> {
        if self.route.is_none() {
            return Some((
                format!(
                    "{} {} · {}",
                    self.sym("⚠"),
                    tr(self.locale, MessageId::LaunchNoModelConnected),
                    tr(self.locale, MessageId::LaunchRunCommand).replace("{command}", "/provider"),
                ),
                ChromeInk::Attention,
            ));
        }
        let mcp = self.mcp.filter(|mcp| mcp.has_news())?;
        let mut parts = Vec::new();
        if mcp.connected > 0 {
            parts.push(count_phrase(
                self.locale,
                mcp.connected,
                MessageId::LaunchMcpConnectedOne,
                MessageId::LaunchMcpConnectedMany,
            ));
        }
        if mcp.needs_sign_in > 0 {
            parts.push(count_phrase(
                self.locale,
                mcp.needs_sign_in,
                MessageId::LaunchMcpNeedsSignInOne,
                MessageId::LaunchMcpNeedsSignInMany,
            ));
            parts.push(tr(self.locale, MessageId::LaunchRunCommand).replace("{command}", "/mcp"));
        }
        Some((
            format!("{} {}", self.sym("●"), parts.join(" · ")),
            ChromeInk::MetadataValue,
        ))
    }
}

fn count_phrase(locale: Locale, count: usize, one: MessageId, many: MessageId) -> String {
    if count == 1 {
        tr(locale, one).into_owned()
    } else {
        tr(locale, many).replace("{count}", &count.to_string())
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

/// The stage's row budget: the header block owns the top, the composer dock
/// the bottom rows (at most four — `[input, hint, rule, prompt]`; the prompt
/// row is the stage's transient status line). Render and hitboxes must
/// agree, so the arithmetic lives here.
struct StartupLayout {
    header: Rect,
    dock: Rect,
}

fn startup_layout(stage: Rect) -> StartupLayout {
    let dock_h = stage.height.saturating_sub(HEADER_ROWS).min(4);
    let dock = Rect {
        y: stage.y + stage.height - dock_h,
        height: dock_h,
        ..stage
    };
    let header = Rect {
        height: stage.height - dock_h,
        ..stage
    };
    StartupLayout { header, dock }
}

/// One content row inside the launch card below the title/announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchCardPlanRow {
    /// Non-interactive `Recent` section heading — no hitbox.
    Heading,
    /// Non-interactive empty-workspace note — no hitbox.
    Note,
    /// Interactive row by index into [`launch_card_rows`].
    Interactive(usize),
}

/// The launch card's laid-out geometry: the card rect plus the absolute y
/// of each content row below the title/announcement.
struct LaunchCardPlan {
    card: Rect,
    announcement: bool,
    rows: Vec<(u16, LaunchCardPlanRow)>,
}

/// The mark rung the card would like on this stage, before knowing whether
/// the interior can hold it. Width alone picks Small vs Tiny for braille;
/// the bitmap tiers have one size.
fn launch_mark_rung(stage: Rect, tier: MarkTier) -> (MarkTier, MarkSize) {
    let margin = (stage.width / 10).clamp(2, 12);
    let card_w = stage.width.saturating_sub(margin.saturating_mul(2));
    let rung = if card_w < TINY_MARK_BELOW_WIDTH {
        MarkSize::Tiny
    } else {
        MarkSize::Small
    };
    (tier, rung)
}

/// Step the mark down until it fits `interior_h` rows: bitmap → braille
/// Small → braille Tiny → none. Each step keeps the whale on the card at
/// the largest size the stage allows.
fn step_mark_to_fit(tier: MarkTier, rung: MarkSize, interior_h: u16) -> (MarkTier, MarkSize) {
    let mut tier = tier;
    let mut rung = rung;
    loop {
        let rows = match tier {
            MarkTier::Image | MarkTier::Sixel => crate::tui::mark::MARK_IMAGE_ROWS,
            MarkTier::Braille => rung.cells().1,
            MarkTier::None => return (tier, rung),
        };
        if interior_h >= rows {
            return (tier, rung);
        }
        match (tier, rung) {
            (MarkTier::Image | MarkTier::Sixel, _) => tier = MarkTier::Braille,
            (MarkTier::Braille, MarkSize::Small) => rung = MarkSize::Tiny,
            (MarkTier::Braille, MarkSize::Tiny) => tier = MarkTier::None,
            (MarkTier::None, _) => return (tier, rung),
        }
    }
}

/// Lay out the launch card: title, one announcement line when true, then
/// the new-session entry, the `Recent` heading over the recents, the
/// see-all overflow, and the empty note when there is no recent work.
/// Pure geometry shared by the painter and
/// [`tideline_startup_row_hitboxes`], so rects match painted cells
/// wherever both run on the same stage.
/// What the launch card has to place: counted once by the caller so the
/// painter and the hitbox pass hand the plan the same inputs.
#[derive(Debug, Clone, Copy)]
struct LaunchCardContent {
    notice_rows: u16,
    announcement: bool,
    interactive: usize,
    has_recents: bool,
    empty_note: bool,
    mark_rows: u16,
}

impl LaunchCardContent {
    fn for_startup(startup: &TidelineStartup<'_>, stage: Rect, interactive: usize) -> Self {
        let (mark_tier, braille_rung) = launch_mark_rung(stage, startup.mark);
        let mark_rows = match mark_tier {
            MarkTier::Image | MarkTier::Sixel => crate::tui::mark::MARK_IMAGE_ROWS,
            MarkTier::Braille => braille_rung.cells().1,
            MarkTier::None => 0,
        };
        Self {
            notice_rows: u16::from(startup.notice.is_some()),
            announcement: startup.state_line().is_some(),
            interactive,
            has_recents: !startup.recent.is_empty(),
            empty_note: startup.recent.is_empty() && !startup.has_more_recent,
            mark_rows,
        }
    }
}

fn launch_card_plan(
    stage: Rect,
    layout: &StartupLayout,
    content: LaunchCardContent,
) -> Option<LaunchCardPlan> {
    let LaunchCardContent {
        notice_rows,
        announcement,
        interactive,
        has_recents,
        empty_note,
        mark_rows,
    } = content;
    let margin = (stage.width / 10).clamp(2, 12);
    let card_w = stage.width.saturating_sub(margin.saturating_mul(2));
    // Vertically centred between the top line (and the notice row it keeps
    // clear) and the composer dock.
    let available = layout
        .dock
        .y
        .saturating_sub(stage.y)
        .saturating_sub(1 + notice_rows);
    if card_w < 20 {
        return None;
    }
    // The card sheds rather than clips: recents and the overflow from the
    // bottom, then the heading/note, then the announcement; the title and
    // the new-session entry hold last. A stage too small even for those
    // keeps only the composer.
    let mut plan_rows: Vec<LaunchCardPlanRow> = vec![LaunchCardPlanRow::Interactive(0)];
    if has_recents {
        plan_rows.push(LaunchCardPlanRow::Heading);
        for index in 1..interactive {
            plan_rows.push(LaunchCardPlanRow::Interactive(index));
        }
    } else if interactive > 1 {
        // No recents: every row past the new-session entry is the see-all
        // overflow.
        for index in 1..interactive {
            plan_rows.push(LaunchCardPlanRow::Interactive(index));
        }
    }
    if empty_note {
        plan_rows.push(LaunchCardPlanRow::Note);
    }
    let mut show_announcement = announcement;
    let mut content_rows = 1 + u16::from(show_announcement) + plan_rows.len() as u16;
    while available < content_rows + 2 {
        if plan_rows
            .last()
            .is_some_and(|last| *last != LaunchCardPlanRow::Interactive(0))
        {
            plan_rows.pop();
            content_rows -= 1;
            continue;
        }
        if show_announcement {
            show_announcement = false;
            content_rows -= 1;
        } else {
            return None;
        }
    }
    // The mark is part of the card, not a decoration that fits when it
    // happens to: reserve its rows whenever the stage can spare them, so a
    // two-row card (title + new session) still carries the whale. Text rows
    // keep their top-aligned positions inside the taller interior.
    let interior_rows = if available >= mark_rows + 2 {
        content_rows.max(mark_rows)
    } else {
        content_rows
    };
    let card_h = interior_rows + 2;
    let card = Rect {
        x: stage.x + margin,
        y: stage.y + 1 + notice_rows + (available - card_h) / 2,
        width: card_w,
        height: card_h,
    };
    let mut rows = Vec::with_capacity(plan_rows.len());
    let mut y = card.y + 1 + u16::from(show_announcement);
    for kind in plan_rows {
        y += 1;
        rows.push((y, kind));
    }
    Some(LaunchCardPlan {
        card,
        announcement: show_announcement,
        rows,
    })
}

/// Mix a style's ink toward the surface colour by `fade` — the card
/// dissolve's whole motion, one bounded lerp.
fn faded(style: Style, theme: &UiTheme, fade: f32) -> Style {
    if fade <= 0.0 {
        return style;
    }
    match style.fg {
        Some(fg) => {
            let mixed = crate::tui::mark::lerp_color(fg, theme.surface_bg, fade);
            Style {
                fg: Some(mixed),
                ..style
            }
        }
        None => style,
    }
}

/// Paint the stage's top line: `⑂ branch  path` left, dim; when the working
/// screen is up, `⋮ MCP n/m` right (MCP status has no other owner).
fn render_launch_top_line(
    area: Rect,
    buf: &mut Buffer,
    startup: &TidelineStartup<'_>,
    card_gone: bool,
) {
    if area.height == 0 {
        return;
    }
    let theme = startup.theme;
    let branch_w = startup
        .branch
        .as_deref()
        .map_or(0, |b| b.width() + usize::from(!b.is_empty()) * 2);
    let prefix_w = startup.sym("⑂").width() + usize::from(!startup.sym("⑂").is_empty()) + branch_w;
    let right_w = card_gone
        .then(|| startup.mcp.filter(|mcp| mcp.enabled > 0))
        .flatten()
        .map_or(0, |_| "⋮ MCP 99/99".width() + 2);
    let budget = usize::from(area.width)
        .saturating_sub(right_w)
        .saturating_sub(prefix_w);
    let workspace = [2usize, 1]
        .into_iter()
        .map(|keep| shorten_workspace(&startup.workspace, keep))
        .fold(startup.workspace.clone(), |chosen, shorter| {
            if chosen.width() > budget {
                shorter
            } else {
                chosen
            }
        });
    let mut left = startup.sym("⑂");
    match startup.branch.as_deref().filter(|b| !b.is_empty()) {
        Some(branch) => {
            left.push(' ');
            left.push_str(branch);
            left.push_str("  ");
        }
        None => left.push(' '),
    }
    left.push_str(&workspace);
    let right = card_gone
        .then(|| startup.mcp.filter(|mcp| mcp.enabled > 0))
        .flatten()
        .map(|mcp| {
            (
                format!("{} MCP {}/{}", startup.sym("⋮"), mcp.connected, mcp.enabled),
                chrome(theme, ChromeInk::Metadata),
            )
        });
    let left_budget = usize::from(area.width).saturating_sub(right_w);
    set_span(
        buf,
        area.x,
        area.y,
        &Span::styled(
            truncate_to_width(&left, left_budget),
            chrome(theme, ChromeInk::MetadataDim),
        ),
    );
    if let Some((text, style)) = right {
        let x = area.right().saturating_sub(text.width() as u16).max(area.x);
        set_span(buf, x, area.y, &Span::styled(text, style));
    }
}

/// Paint the centred launch card: the mark at left, `Codewhale` + version,
/// one announcement line only when true, then the prominent new-session
/// entry over the recent-work list (PRD 4.1). The dissolve fades every ink
/// toward the surface colour; at progress 1.0 the caller stops painting
/// the card entirely. Returns the sixel tier's reserved block (stage
/// coordinates), or a zero-width rect when no sixel block was reserved.
fn render_launch_card(
    stage: Rect,
    buf: &mut Buffer,
    startup: &TidelineStartup<'_>,
    layout: &StartupLayout,
) -> Rect {
    let theme = startup.theme;
    let fade = startup.card_dissolve;
    let rows = launch_card_rows(startup.locale, &startup.recent, startup.has_more_recent);
    let announcement = startup.state_line();
    let (mark_tier, braille_rung) = launch_mark_rung(stage, startup.mark);
    let Some(plan) = launch_card_plan(
        stage,
        layout,
        LaunchCardContent::for_startup(startup, stage, rows.len()),
    ) else {
        return Rect::new(0, 0, 0, 0);
    };
    let card = plan.card;
    let card_w = card.width;

    let border = faded(chrome(theme, ChromeInk::MetadataDim), theme, fade);
    let top = startup.sym(&{
        let mut text = String::from("╭");
        text.push_str(&"─".repeat(usize::from(card_w.saturating_sub(2))));
        text.push('╮');
        text
    });
    set_span(buf, card.x, card.y, &Span::styled(top, border));
    let bar = startup.sym("│");
    for row in 1..card.height.saturating_sub(1) {
        set_span(
            buf,
            card.x,
            card.y + row,
            &Span::styled(bar.clone(), border),
        );
        set_span(
            buf,
            card.right().saturating_sub(1),
            card.y + row,
            &Span::styled(bar.clone(), border),
        );
    }
    let bottom = startup.sym(&{
        let mut text = String::from("╰");
        text.push_str(&"─".repeat(usize::from(card_w.saturating_sub(2))));
        text.push('╯');
        text
    });
    set_span(
        buf,
        card.x,
        card.y + card.height.saturating_sub(1),
        &Span::styled(bottom, border),
    );

    // The mark: the card's left column, vertically centred in the interior.
    // Only the sixel tier reports a block; every other tier leaves this
    // zero-width so the event loop emits nothing.
    let mut sixel_reserve = Rect::new(0, 0, 0, 0);
    let interior_h = card.height.saturating_sub(2);
    // The plan reserved rows for this rung; if the stage could not spare
    // them, step down (Image/Sixel → braille Small → Tiny) before giving
    // the mark up. No-mark is the last resort, never the first.
    let (mark_tier, braille_rung) = step_mark_to_fit(mark_tier, braille_rung, interior_h);
    let (mark_cols, mark_rows) = match mark_tier {
        MarkTier::Image | MarkTier::Sixel => (
            crate::tui::mark::MARK_IMAGE_COLS,
            crate::tui::mark::MARK_IMAGE_ROWS,
        ),
        MarkTier::Braille => braille_rung.cells(),
        MarkTier::None => (0, 0),
    };
    let mark_fits = mark_tier != MarkTier::None && card_w >= 30 && interior_h >= mark_rows;
    let text_x = if mark_fits {
        let mark_area = Rect {
            x: card.x + 2,
            y: card.y + 1 + (interior_h - mark_rows) / 2,
            width: mark_cols,
            height: mark_rows,
        };
        match mark_tier {
            MarkTier::Image => {
                crate::tui::mark::render_kitty_placeholders(
                    mark_area,
                    buf,
                    startup.surface_progress,
                );
            }
            MarkTier::Sixel => {
                // Binary visibility, no surfacing: the raster is either
                // there (settled, like reduced motion) or gone. Once the
                // card starts dissolving the block stays unreserved so the
                // event loop clears the image instead of stranding it over
                // the working screen.
                if fade <= 0.0 {
                    sixel_reserve = crate::tui::mark::render_sixel_reserve(mark_area, buf);
                }
            }
            MarkTier::Braille => {
                crate::tui::mark::render_mark(
                    mark_area,
                    buf,
                    braille_rung,
                    crate::tui::mark::lerp_color(
                        theme.surface_bg,
                        theme.accent_action,
                        startup.surface_progress,
                    ),
                    theme.surface_bg,
                    startup.surface_progress,
                );
            }
            MarkTier::None => {}
        }
        mark_area.x + mark_cols + HEADER_GUTTER
    } else {
        card.x + 2
    };

    let interior_w = usize::from(card.right().saturating_sub(1).saturating_sub(text_x));
    let fit = |text: &str| truncate_to_width(text, interior_w);
    let mut row = card.y + 1;

    // Title: the wordmark bold, the version dim.
    set_span(
        buf,
        text_x,
        row,
        &Span::styled(
            fit("codewhale"),
            faded(
                Style::default()
                    .fg(theme.accent_action)
                    .add_modifier(Modifier::BOLD),
                theme,
                fade,
            ),
        ),
    );
    let version = format!("v{}", startup.version);
    let version_x = text_x + "codewhale".width() as u16 + 1;
    if usize::from(version_x) + version.width() <= interior_w {
        set_span(
            buf,
            version_x,
            row,
            &Span::styled(
                version,
                faded(chrome(theme, ChromeInk::MetadataHint), theme, fade),
            ),
        );
    }
    row += 1;

    // The announcement: the one blocking fact or piece of news, only true
    // (and only when the plan kept it).
    if plan.announcement
        && let Some((line, ink)) = announcement
    {
        set_span(
            buf,
            text_x,
            row,
            &Span::styled(fit(&line), faded(chrome(theme, ink), theme, fade)),
        );
        row += 1;
    }
    debug_assert_eq!(
        row,
        plan.rows.first().map_or(row, |(y, _)| *y),
        "title/announcement rows must land on the plan"
    );

    // The rows: ↑/↓ highlight, Enter runs the highlighted row, hover
    // paints the same shared selected-row treatment as the keyboard, and
    // recent details sit right-aligned where the chords used to be.
    // Nothing is highlighted until the user arrows or hovers.
    let right_edge = card.right().saturating_sub(2);
    for (y, kind) in &plan.rows {
        match kind {
            LaunchCardPlanRow::Heading => {
                set_span(
                    buf,
                    text_x,
                    *y,
                    &Span::styled(
                        fit(&tr(startup.locale, MessageId::LaunchRecentHeading)),
                        faded(chrome(theme, ChromeInk::MetadataDim), theme, fade),
                    ),
                );
            }
            LaunchCardPlanRow::Note => {
                set_span(
                    buf,
                    text_x,
                    *y,
                    &Span::styled(
                        fit(&tr(startup.locale, MessageId::LaunchNoRecentSessions)),
                        faded(chrome(theme, ChromeInk::Metadata), theme, fade),
                    ),
                );
            }
            LaunchCardPlanRow::Interactive(index) => {
                let Some(entry) = rows.get(*index) else {
                    continue;
                };
                let active =
                    startup.menu_selected == Some(*index) || startup.hovered == Some(*index);
                let marker = startup.sym(crate::tui::glyphs::selection_marker(active));
                let marker_w = marker.width() as u16;
                // The selected/hovered band runs the row's full interior in
                // the shared selection treatment (the pickers' convention);
                // the prominent new-session entry reads bold accent until
                // then.
                let row_style = if active {
                    faded(crate::tui::menu_style::selected_row_style(), theme, fade)
                } else if entry.prominent {
                    faded(
                        Style::default()
                            .fg(theme.accent_action)
                            .add_modifier(Modifier::BOLD),
                        theme,
                        fade,
                    )
                } else {
                    faded(chrome(theme, ChromeInk::Metadata), theme, fade)
                };
                if active {
                    let fill = crate::tui::menu_style::selected_row_bg_style();
                    let mut x = card.x + 1;
                    while x < card.right().saturating_sub(1) {
                        let cell = &mut buf[(x, *y)];
                        if let Some(bg) = faded(fill, theme, fade).bg {
                            cell.set_bg(bg);
                        }
                        x += 1;
                    }
                }
                set_span(buf, text_x, *y, &Span::styled(marker.clone(), row_style));
                set_span(
                    buf,
                    text_x + marker_w + 1,
                    *y,
                    &Span::styled(fit(&entry.label), row_style),
                );
                if !entry.detail.is_empty() {
                    let detail_x = right_edge.saturating_sub(entry.detail.width() as u16);
                    let label_end = text_x + marker_w + 1 + entry.label.width() as u16 + 1;
                    if detail_x > label_end {
                        set_span(
                            buf,
                            detail_x,
                            *y,
                            &Span::styled(entry.detail.clone(), row_style),
                        );
                    }
                }
            }
        }
    }
    sixel_reserve
}

/// Paint the startup stage: top line, the launch card (or the working
/// screen once dissolved), then the docked composer. Deterministic; every
/// fact is injected. Returns the sixel tier's reserved block (stage
/// coordinates), or a zero-width rect when no sixel block was reserved.
pub fn render_tideline_startup(
    stage: Rect,
    buf: &mut Buffer,
    startup: &TidelineStartup<'_>,
) -> Rect {
    if stage.width < 8 || stage.height < 5 {
        return Rect::new(0, 0, 0, 0);
    }
    let theme = startup.theme;
    let layout = startup_layout(stage);

    // The top line: ⑂ branch  path, one dim row the terminal owns above the
    // stage. The working screen (card gone) gains the `⋮ MCP n/m` chip right.
    let card_gone = startup.card_dissolve >= 1.0;
    render_launch_top_line(layout.header, buf, startup, card_gone);

    // The sixel block reserved by this paint, if any. The card is its only
    // source; the working screen and the composer never reserve.
    let mut sixel_reserve = Rect::new(0, 0, 0, 0);
    if card_gone {
        // The working screen's first transcript receipt, only when the
        // session fact is true.
        let receipt = if startup.session_hooks > 0 {
            format!(
                "{} session_start {} {}",
                startup.sym("◆"),
                startup.sym("·"),
                tr(startup.locale, MessageId::ReceiptSessionHooks)
                    .replace("{count}", &startup.session_hooks.to_string()),
            )
        } else {
            format!("{} session_start", startup.sym("◆"))
        };
        set_span(
            buf,
            layout.header.x + HEADER_MARGIN,
            layout.header.y + 2,
            &Span::styled(receipt, chrome(theme, ChromeInk::Metadata)),
        );
    } else {
        sixel_reserve = render_launch_card(stage, buf, startup, &layout);
    }

    // The docked pre-session composer is the same rounded Tideline shell
    // used by the work surface whenever the full four-row dock fits. Its
    // shell owns both the visible `[↑]` affordance and the matching
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
    // The one migration notice above the composer, only when true.
    if !card_gone
        && let Some(notice) = startup.notice.as_deref()
        && layout.dock.y > stage.y
    {
        let x = stage.x + HEADER_MARGIN + 1;
        set_span(
            buf,
            x,
            layout.dock.y.saturating_sub(1),
            &Span::styled(
                truncate_to_width(
                    notice,
                    usize::from(stage.width.saturating_sub(HEADER_MARGIN * 2 + 2)),
                ),
                chrome(theme, ChromeInk::MetadataDim),
            ),
        );
    }
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
            (!card_gone)
                .then_some(startup.composer_rule.as_deref())
                .flatten(),
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
    sixel_reserve
}

/// Recorded interactive hitboxes for the startup stage: the docked
/// composer's focus, input and send targets, plus the launch card's
/// clickable rows. The header is deliberately non-interactive and has no
/// hitbox.
#[derive(Debug, Clone, Default)]
pub struct TidelineStartupHitboxes {
    /// The docked composer focus surface (a click here is a no-op that keeps
    /// focus where it already is).
    pub composer: Option<Rect>,
    /// The actual text-input row; completion menus anchor directly above it.
    pub input: Option<Rect>,
    /// The send glyph inside the composer row (click submits).
    pub send: Option<Rect>,
    /// The card's clickable rows in [`launch_card_rows`] order.
    pub rows: Vec<(crate::tui::app::LaunchRowId, Rect)>,
}

/// Clickable rects for the startup card's rows: the same
/// [`launch_card_plan`] geometry the painter uses, so rects match painted
/// cells wherever both run on the same stage.
#[must_use]
pub fn tideline_startup_row_hitboxes(
    stage: Rect,
    startup: &TidelineStartup<'_>,
) -> Vec<(crate::tui::app::LaunchRowId, Rect)> {
    let rows = launch_card_rows(startup.locale, &startup.recent, startup.has_more_recent);
    let layout = startup_layout(stage);
    let Some(plan) = launch_card_plan(
        stage,
        &layout,
        LaunchCardContent::for_startup(startup, stage, rows.len()),
    ) else {
        return Vec::new();
    };
    plan.rows
        .iter()
        .filter_map(|(y, kind)| match kind {
            LaunchCardPlanRow::Interactive(index) => rows.get(*index).map(|row| {
                (
                    row.id.clone(),
                    Rect {
                        x: plan.card.x + 1,
                        y: *y,
                        width: plan.card.width.saturating_sub(2),
                        height: 1,
                    },
                )
            }),
            LaunchCardPlanRow::Heading | LaunchCardPlanRow::Note => None,
        })
        .collect()
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

/// Project live `App` state onto the startup stage's inputs: the route the
/// info line reports, the workspace the git probe observed, the MCP
/// snapshot, the composer, and the launch-relative ambient clock.
#[must_use]
pub fn tideline_startup_from_app(app: &App) -> TidelineStartup<'_> {
    let ascii_safe = crate::tui::color_compat::ascii_safe_enabled();
    // The route facts are the info line's own, already shed to a budget;
    // the renderer truncates to the stage on top of that.
    let route = (!app.onboarding_needs_api_key)
        .then(|| crate::tui::phase_strip::route_identity_fields(app, ShellTier::Wide, ROUTE_BUDGET))
        .flatten()
        .map(|fields| fields.join(FIELD_JOIN));
    // Repository and branch come from the one cached `git_status` snapshot —
    // the render path never probes.
    let git = crate::tui::git_status::cached_status();
    let git_matches_workspace = git.probed_workspace.as_deref() == Some(app.workspace.as_path());
    let workspace = match git.remote_slug.as_deref().filter(|_| git_matches_workspace) {
        Some(slug) if !slug.is_empty() => match git.branch.as_deref() {
            Some(branch) if !branch.is_empty() => format!("{slug}{FIELD_JOIN}{branch}"),
            _ => slug.to_string(),
        },
        _ => app.workspace.display().to_string(),
    };
    let mark = if ascii_safe {
        MarkTier::None
    } else if crate::tui::mark::kitty_graphics_supported() {
        MarkTier::Image
    } else if app.use_alt_screen()
        && app.launch.sixel_cell_px.is_some()
        && crate::tui::mark::sixel_graphics_supported()
        && crate::tui::mark::sixel_field_bg(&app.ui_theme, app.launch.sixel_terminal_bg).is_some()
    {
        // Sixel last: cursor-addressed pixels need the alternate screen
        // (inline viewports have no stable CUP origin), a measured cell
        // size, and an RGB field to composite the raster's corners onto.
        // Anything missing keeps the braille tier.
        MarkTier::Sixel
    } else {
        MarkTier::Braille
    };
    // The composer's launch rule: `model (effort) · permission` — the one
    // place the route and the posture show while the card is up — plus the
    // filesystem scope whenever it says something the permission word does
    // not (PRD 4.1: the recommended route carries its visible trust and
    // billing boundary; the provider identity in the route line is the
    // billing owner, the permission + scope is the trust boundary).
    let (_, model) = app.effective_route_identity_display();
    let permission = permission_label(app);
    let scope = filesystem_scope_notice(app).map(|scope| scope.into_owned());
    let mut rule = if model.is_empty() {
        format!(
            "{} · {}",
            tr(app.ui_locale, MessageId::InfoLineNotConnected),
            permission
        )
    } else {
        let effort = app.reasoning_effort_display_label();
        if effort.is_empty() {
            format!("{model} · {permission}")
        } else {
            format!("{model} ({effort}) · {permission}")
        }
    };
    if let Some(scope) = scope {
        rule.push_str(" · ");
        rule.push_str(&scope);
    }
    let composer_rule = Some(rule);
    // Recent work is projected from the launch state's loaded list — the
    // render path never touches disk.
    let (recent, has_more) = launch_recent_entries(app);
    let branch = git_matches_workspace
        .then(|| git.branch.clone())
        .flatten()
        .filter(|branch| !branch.is_empty());
    let session_hooks = if app.hooks.config().enabled {
        app.hooks
            .config()
            .hooks
            .iter()
            .filter(|hook| hook.event == crate::hooks::HookEvent::SessionStart)
            .count()
    } else {
        0
    };
    let dissolve_motion = app.motion_policy().allows_decorative() && !app.low_motion;
    TidelineStartup::new(&app.ui_theme, route, workspace)
        .locale(app.ui_locale)
        .mcp(app.mcp_snapshot.as_ref().map(McpFacts::from_snapshot))
        .ascii_safe(ascii_safe)
        .mark(mark)
        .composer(LaunchComposerDisplay::from_app(app))
        // The transient line over the dock: the latest launch status (a
        // resume failure leaves the card up and says why here).
        .status_line(app.launch.status.clone())
        .recent(recent, has_more)
        .menu_selected(app.launch.menu_selected)
        .hovered(app.launch.hovered_row)
        .notice(
            app.launch
                .claude_code_detected
                .then(|| tr(app.ui_locale, MessageId::LaunchNoticeClaude).into_owned()),
        )
        .composer_rule(composer_rule)
        .branch(branch)
        .session_hooks(session_hooks)
        .card_dissolve(
            app.launch
                .card_dissolve_progress(app.ambient_clock_ms, dissolve_motion),
        )
        // The app opens on this screen, so the ambient clock is already
        // launch-relative. Reduced motion asks for the settled mark, which is
        // the same drawing at its endpoint.
        .surface_progress(if app.motion_policy().allows_decorative() {
            crate::tui::mark::surface_progress(app.ambient_clock_ms, MARK_SURFACE_MS)
        } else {
            1.0
        })
}

/// Store the startup stage's clickable rects into the launch state. Call
/// after the stage is painted, with the hitboxes computed for the same
/// stage rect: the docked composer's input and send rects land in
/// `composer_area`/`send_area`, and the card's clickable rows land in
/// `row_hitboxes` (hover and click share them).
pub fn apply_launch_hitboxes(
    hitboxes: &TidelineStartupHitboxes,
    launch: &mut crate::tui::app::LaunchState,
) {
    launch.composer_area = hitboxes.composer;
    launch.send_area = hitboxes.send;
    launch.row_hitboxes = hitboxes.rows.clone();
    // Hover must match a painted cell, so a shed row clears it; the
    // keyboard selection is intentionally kept (Enter still runs it).
    if launch
        .hovered_row
        .is_some_and(|hovered| hovered >= launch.row_hitboxes.len())
    {
        launch.hovered_row = None;
    }
}

#[cfg(test)]
mod tideline_tests;
