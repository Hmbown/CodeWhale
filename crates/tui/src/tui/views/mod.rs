use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget, Wrap},
};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::fmt;
use unicode_width::UnicodeWidthStr;

use crate::config::{ApiProvider, Config};
use crate::features::{FEATURES, Stage};
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::settings::Settings;
use crate::tools::UserInputResponse;
use crate::tools::subagent::{SubAgentAssignment, SubAgentResult, SubAgentStatus, SubAgentType};
use crate::tui::app::App;
use crate::tui::approval::{ElevationOption, ReviewDecision};
use crate::tui::history::{HistoryCell, SubAgentCell, summarize_tool_output};
use crate::tui::widgets::agent_card::AgentLifecycle;

pub mod fleet_roster;
pub mod fleet_setup;
pub mod mode_picker;
pub mod status_picker;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalKind {
    Approval,
    Elevation,
    UserInput,
    PlanPrompt,
    CommandPalette,
    Help,
    SubAgents,
    Pager,
    LiveTranscript,
    SessionPicker,
    Config,
    ModelPicker,
    ProviderPicker,
    ModePicker,
    FleetRoster,
    FleetSetup,
    HotbarSetup,
    SetupWizard,
    FilePicker,
    StatusPicker,
    FeedbackPicker,
    ThemePicker,
    ContextMenu,
}

/// Clear and paint a modal popup with an opaque surface.
///
/// Older modals often called `Clear` only, which left reset-background blank
/// cells that could read as translucent on terminals with a non-default app
/// background. This helper makes the popup area explicit and keeps the small
/// shadow from inheriting stale transcript glyphs.
pub(crate) fn render_modal_surface(area: Rect, popup_area: Rect, buf: &mut Buffer) {
    let shadow_x = popup_area.x.saturating_add(1);
    let shadow_y = popup_area.y.saturating_add(1);
    let shadow_right = area.x.saturating_add(area.width);
    let shadow_bottom = area.y.saturating_add(area.height);
    let shadow_width = popup_area.width.min(shadow_right.saturating_sub(shadow_x));
    let shadow_height = popup_area
        .height
        .min(shadow_bottom.saturating_sub(shadow_y));

    if shadow_width > 0 && shadow_height > 0 {
        Block::default()
            .style(Style::default().bg(palette::SURFACE_ELEVATED))
            .render(
                Rect {
                    x: shadow_x,
                    y: shadow_y,
                    width: shadow_width,
                    height: shadow_height,
                },
                buf,
            );
    }

    Clear.render(popup_area, buf);
    Block::default()
        .style(Style::default().bg(palette::WHALE_BG))
        .render(popup_area, buf);
}

fn render_modal_backdrop(area: Rect, buf: &mut Buffer) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)]
                .set_symbol(" ")
                .set_style(Style::default().bg(palette::WHALE_BG));
        }
    }
}

/// Compute a centered, responsive popup rect for a modal.
///
/// The size starts from `preferred_*`, but is clamped so it never exceeds the
/// frame (leaving a small breathing-room margin when there is space) and never
/// drops below `min_*` unless the frame itself is smaller. Centering the result
/// inside `area` replaces the repeated, error-prone
/// `N.min(area.width.saturating_sub(..))` arithmetic scattered across modals so
/// every overlay sizes itself the same way at 80x24, 100x30, 120x32, 160x40,
/// and beyond. See #3732.
pub(crate) fn centered_modal_area(
    area: Rect,
    preferred_width: u16,
    preferred_height: u16,
    min_width: u16,
    min_height: u16,
) -> Rect {
    // Keep a 2-cell margin on each axis when the frame can spare it so the
    // backdrop stays visible around the card; otherwise fill the frame.
    let avail_width = area.width.saturating_sub(2).max(1);
    let avail_height = area.height.saturating_sub(2).max(1);
    let width = preferred_width.clamp(min_width.min(avail_width), avail_width);
    let height = preferred_height.clamp(min_height.min(avail_height), avail_height);
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// A single key/label hint shown in a modal's action footer.
///
/// Footers built from `ActionHint`s are laid out by [`action_footer_lines`],
/// which wraps to additional rows instead of letting an action run off the
/// right edge of the modal — the core overflow bug behind #3732. Use this for
/// action/navigation hints; truncate only identifiers/paths/hashes elsewhere.
pub(crate) struct ActionHint {
    key: Cow<'static, str>,
    label: Cow<'static, str>,
}

impl ActionHint {
    pub(crate) fn new(
        key: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
        }
    }

    /// Display columns this hint occupies: ` key ` (key padded by a space on
    /// each side) followed by the label.
    fn width(&self) -> usize {
        UnicodeWidthStr::width(self.key.as_ref()) + 2 + UnicodeWidthStr::width(self.label.as_ref())
    }

    fn spans(&self) -> [Span<'static>; 2] {
        [
            Span::styled(
                format!(" {} ", self.key),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                self.label.clone().into_owned(),
                Style::default().fg(palette::TEXT_MUTED),
            ),
        ]
    }
}

/// Lay out action hints into one or more lines that each fit within `width`.
///
/// Hints are packed greedily; when the next hint would overflow the current row
/// the layout starts a new row rather than truncating. No action is ever
/// dropped or clipped (a single hint wider than `width` is emitted alone, which
/// only happens at degenerate widths below the modal minimums). This is the
/// shared replacement for the single-line `title_bottom` footers that silently
/// pushed actions off-screen.
pub(crate) fn action_footer_lines(hints: &[ActionHint], width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if hints.is_empty() || width == 0 {
        return Vec::new();
    }
    const GAP: usize = 1;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;
    for hint in hints {
        let hint_width = hint.width();
        let needed = if current.is_empty() {
            hint_width
        } else {
            current_width + GAP + hint_width
        };
        if !current.is_empty() && needed > width {
            lines.push(Line::from(std::mem::take(&mut current)));
            current_width = 0;
        }
        if !current.is_empty() {
            current.push(Span::raw(" ".repeat(GAP)));
            current_width += GAP;
        }
        current.extend(hint.spans());
        current_width += hint_width;
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

/// Reserve `lines` worth of rows at the bottom of `inner`, paint them, and
/// return the content area that remains above. Shared by the action-hint and
/// free-text modal footers.
fn place_footer_lines(inner: Rect, buf: &mut Buffer, lines: Vec<Line<'static>>) -> Rect {
    if lines.is_empty() || inner.height == 0 {
        return inner;
    }
    let footer_height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(inner.height);
    let footer_area = Rect {
        x: inner.x,
        y: inner.y + inner.height - footer_height,
        width: inner.width,
        height: footer_height,
    };
    Paragraph::new(lines).render(footer_area, buf);
    Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height - footer_height,
    }
}

/// Render a wrapping action footer anchored to the bottom of `inner` and
/// return the content area that remains above it.
///
/// Modals call this after painting their block so the footer reserves exactly
/// as many rows as it needs (bounded by the available height) and the body
/// fills the rest. Centralizing it keeps every modal's action row visible and
/// reachable at narrow widths.
pub(crate) fn render_modal_footer(inner: Rect, buf: &mut Buffer, hints: &[ActionHint]) -> Rect {
    let lines = action_footer_lines(hints, inner.width);
    place_footer_lines(inner, buf, lines)
}

/// Word-wrap a free-form footer string into styled lines that each fit `width`.
///
/// For footers that are pre-composed prose/sentences (e.g. localized config
/// hints) rather than discrete key/label hints. Wrapping on whitespace keeps
/// every word visible instead of clipping the tail at the modal edge.
pub(crate) fn wrapped_footer_lines(text: &str, width: u16, style: Style) -> Vec<Line<'static>> {
    let width = usize::from(width);
    if text.trim().is_empty() || width == 0 {
        return Vec::new();
    }
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;
    for word in text.split_whitespace() {
        let word_width = UnicodeWidthStr::width(word);
        let needed = if current.is_empty() {
            word_width
        } else {
            current_width + 1 + word_width
        };
        if !current.is_empty() && needed > width {
            lines.push(Line::from(Span::styled(
                std::mem::take(&mut current),
                style,
            )));
            current_width = 0;
        }
        if !current.is_empty() {
            current.push(' ');
            current_width += 1;
        }
        current.push_str(word);
        current_width += word_width;
    }
    if !current.is_empty() {
        lines.push(Line::from(Span::styled(current, style)));
    }
    lines
}

/// Render a wrapping free-text footer anchored to the bottom of `inner` and
/// return the content area above it. The prose counterpart to
/// [`render_modal_footer`].
pub(crate) fn render_modal_text_footer(
    inner: Rect,
    buf: &mut Buffer,
    text: &str,
    style: Style,
) -> Rect {
    let lines = wrapped_footer_lines(text, inner.width, style);
    place_footer_lines(inner, buf, lines)
}

/// Shared list/detail geometry for modal managers and pickers.
///
/// Wide modals get a stable left list and a right detail pane. Narrow modals
/// stack the list over the detail so neither side becomes unreadably thin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ListDetailLayout {
    pub(crate) list: Rect,
    pub(crate) detail: Rect,
    pub(crate) stacked: bool,
}

impl ListDetailLayout {
    #[must_use]
    pub(crate) fn split(area: Rect, min_detail_width: u16) -> Self {
        if area.width == 0 || area.height == 0 {
            return Self {
                list: area,
                detail: area,
                stacked: true,
            };
        }

        let gap = 1;
        let min_list_width = 30.min(area.width);
        let can_split = area.width >= 96
            && area
                .width
                .saturating_sub(gap)
                .saturating_sub(min_list_width)
                >= min_detail_width;
        if can_split {
            let max_list_width = area.width.saturating_sub(gap + min_detail_width);
            let preferred = area.width.saturating_mul(42) / 100;
            let list_width = preferred.clamp(min_list_width, max_list_width.min(52));
            let detail_width = area.width.saturating_sub(list_width + gap);
            return Self {
                list: Rect {
                    x: area.x,
                    y: area.y,
                    width: list_width,
                    height: area.height,
                },
                detail: Rect {
                    x: area.x + list_width + gap,
                    y: area.y,
                    width: detail_width,
                    height: area.height,
                },
                stacked: false,
            };
        }

        let gap = if area.height >= 8 { 1 } else { 0 };
        let min_detail_height = 4.min(area.height);
        let max_list_height = area.height.saturating_sub(gap + min_detail_height);
        let preferred = area.height.saturating_mul(3) / 5;
        let list_height = preferred.clamp(1, max_list_height.max(1));
        let detail_height = area.height.saturating_sub(list_height + gap);
        Self {
            list: Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: list_height,
            },
            detail: Rect {
                x: area.x,
                y: area.y + list_height + gap,
                width: area.width,
                height: detail_height,
            },
            stacked: true,
        }
    }
}

/// Plain empty-state copy for modal list/detail bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmptyState {
    title: Cow<'static, str>,
    body: Cow<'static, str>,
    primary_action: Option<(Cow<'static, str>, Cow<'static, str>)>,
    secondary_action: Option<(Cow<'static, str>, Cow<'static, str>)>,
}

impl EmptyState {
    pub(crate) fn new(
        title: impl Into<Cow<'static, str>>,
        body: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            primary_action: None,
            secondary_action: None,
        }
    }

    #[must_use]
    pub(crate) fn primary_action(
        mut self,
        key: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.primary_action = Some((key.into(), label.into()));
        self
    }

    #[must_use]
    pub(crate) fn secondary_action(
        mut self,
        key: impl Into<Cow<'static, str>>,
        label: impl Into<Cow<'static, str>>,
    ) -> Self {
        self.secondary_action = Some((key.into(), label.into()));
        self
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let mut lines = vec![
            Line::from(Span::styled(
                self.title.clone().into_owned(),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                self.body.clone().into_owned(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
        ];
        if self.primary_action.is_some() || self.secondary_action.is_some() {
            lines.push(Line::from(""));
        }
        for (key, label) in [self.primary_action.as_ref(), self.secondary_action.as_ref()]
            .into_iter()
            .flatten()
        {
            let hint = ActionHint::new(key.clone(), label.clone());
            lines.push(Line::from(hint.spans().to_vec()));
        }
        Paragraph::new(lines)
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }
}

#[derive(Debug, Clone)]
pub enum CommandPaletteAction {
    ExecuteCommand { command: String },
    InsertText { text: String },
    OpenTextPager { title: String, content: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuAction {
    CopySelection,
    OpenSelection,
    ClearSelection,
    CopyCell {
        cell_index: usize,
    },
    OpenDetails {
        cell_index: usize,
    },
    Paste,
    OpenCommandPalette,
    OpenContextInspector,
    OpenHelp,
    /// Open the selected file:line in the user's editor.
    OpenFileAtLine {
        cell_index: usize,
    },
    /// Hide a transcript cell. Adds the cell's index to `collapsed_cells`.
    HideCell {
        cell_index: usize,
    },
    /// Show a previously hidden cell (when right-clicking near it).
    ShowCell {
        cell_index: usize,
    },
    /// Show all currently hidden cells.
    ShowAllHidden,
    /// Execute a slash command associated with a contextual UI row.
    ExecuteCommand {
        command: String,
    },
    /// Copy a pre-resolved text payload (e.g. a sidebar row's full text)
    /// to the clipboard.
    CopyText {
        text: String,
    },
}

#[derive(Debug, Clone)]
pub enum ViewEvent {
    CommandPaletteSelected {
        action: CommandPaletteAction,
    },
    OpenTextPager {
        title: String,
        content: String,
    },
    ApprovalDecision {
        tool_id: String,
        tool_name: String,
        decision: ReviewDecision,
        timed_out: bool,
        /// Exact-argument fingerprint, used to scope *denials* (#1617).
        approval_key: String,
        /// Lossy / arity-aware fingerprint, used to scope *approvals*.
        approval_grouping_key: String,
        /// Ask-only permission rules to append when the decision approves.
        persistent_ask_rules: Vec<codewhale_config::ToolAskRule>,
    },
    ElevationDecision {
        tool_id: String,
        tool_name: String,
        option: ElevationOption,
    },
    UserInputSubmitted {
        tool_id: String,
        response: UserInputResponse,
    },
    UserInputCancelled {
        tool_id: String,
    },
    ConfigUpdated {
        key: String,
        value: String,
        persist: bool,
    },
    PlanPromptSelected {
        option: usize,
    },
    PlanPromptDismissed,
    SubAgentsRefresh,
    SidebarAgentCancel {
        agent_id: String,
    },
    /// Emitted by the file picker (`Ctrl+P`) when the user presses Enter on a
    /// candidate. The handler should insert `@<path>` at the composer's cursor
    /// position.
    FilePickerSelected {
        path: String,
    },
    SessionSelected {
        session_id: String,
    },
    SessionDeleted {
        session_id: String,
        title: String,
    },
    /// Emitted by the `/model` picker on Enter — carries both the chosen
    /// model id and reasoning effort tier so the UI handler can update App
    /// state, persist via `Settings`, and forward `Op::SetModel` to the
    /// running engine. `previous_*` fields let the handler skip work when
    /// nothing changed and craft a clear status message.
    ModelPickerApplied {
        model: String,
        provider: Option<crate::config::ApiProvider>,
        effort: crate::tui::app::ReasoningEffort,
        previous_model: String,
        previous_effort: crate::tui::app::ReasoningEffort,
    },
    /// Emitted by the `/model` picker on Esc so the next open can restore
    /// the browsing context — view mode and highlighted row (#4109 / #4115).
    ModelPickerDismissed {
        /// True when the dismissed view browses beyond configured providers
        /// (Catalog / Recent / Coding / Cheap / Long context).
        catalog_view: bool,
        /// Named view key (`configured`, `catalog`, `recent`, `coding`,
        /// `cheap`, `long_context`) for reopen restore (#4115).
        view: String,
        selected_row_id: Option<String>,
    },
    /// Emitted by the `/provider` picker on Esc so the next open can restore
    /// the browsing context — view mode and highlighted row.
    ProviderPickerDismissed {
        catalog_view: bool,
        selected_provider_id: Option<String>,
    },
    /// Emitted by the `/provider` picker when the user selects a provider
    /// that already has credentials — the handler should perform the same
    /// switch as `AppAction::SwitchProvider`.
    ProviderPickerApplied {
        provider: crate::config::ApiProvider,
        provider_id: Option<String>,
    },
    /// Emitted by the `/provider` picker after the user types an API key
    /// inline for a provider that lacked one. The handler validates the key
    /// live; on success it reopens the guided flow at the model-pick stage
    /// without persisting yet (#3875).
    ProviderPickerApiKeySubmitted {
        provider: crate::config::ApiProvider,
        provider_id: Option<String>,
        api_key: String,
    },
    /// Emitted by the `/provider` guided setup confirm stage after the user
    /// accepted provider + model. The handler persists the key (and model)
    /// via the comment-preserving config path, then performs the switch.
    ProviderPickerSetupConfirmed {
        provider: crate::config::ApiProvider,
        provider_id: Option<String>,
        api_key: String,
        model: String,
    },
    /// Emitted by the `/provider` picker after the custom provider form is
    /// completed. The handler persists a named OpenAI-compatible provider
    /// table and switches to it without storing raw secrets.
    ProviderPickerCustomProviderSubmitted {
        provider_id: String,
        base_url: String,
        model: Option<String>,
        api_key_env: Option<String>,
    },
    /// Emitted by the `/provider` picker when Kimi CLI OAuth credentials can
    /// be reused for Moonshot/Kimi dispatch.
    ProviderPickerKimiOAuthEnabled {
        provider: crate::config::ApiProvider,
    },
    /// Emitted by the `/provider` picker (the `M` action) to jump straight to
    /// the `/model` picker pre-filtered to the highlighted provider (#3083).
    ProviderPickerOpenModels {
        provider: crate::config::ApiProvider,
        provider_id: Option<String>,
    },
    /// Emitted by the `/mode` picker when the user chooses a mode.
    ModeSelected {
        mode: crate::tui::app::AppMode,
    },
    /// Emitted by the `/statusline` picker every time the user toggles an
    /// item (live preview) and once more on Enter (final). The handler
    /// updates `app.status_items` immediately and persists on `final_save`
    /// so the footer animates without a write per keystroke.
    StatusItemsUpdated {
        items: Vec<crate::config::StatusItem>,
        final_save: bool,
    },
    /// Emitted by the `/hotbar` setup wizard when the user saves the draft
    /// bindings. The host updates live config state; disk persistence is
    /// handled by the follow-up persistence slice.
    HotbarSetupSaved {
        bindings: Vec<codewhale_config::HotbarBindingToml>,
    },
    /// Emitted by the constitution-first setup shell when a staged setup-state
    /// record should be committed atomically to `$CODEWHALE_HOME/setup_state.json`.
    SetupStateCommitRequested {
        state: codewhale_config::SetupState,
        message: String,
    },
    /// Emitted by the constitution-first setup shell when accepting a guided
    /// structured user-global constitution. The host commits the constitution
    /// and matching setup-state record together.
    SetupConstitutionCommitRequested {
        constitution: codewhale_config::UserConstitution,
        state: codewhale_config::SetupState,
        message: String,
    },
    /// Emitted by the setup Constitution card (`A`, provider route ready) to
    /// ask the user's first configured model to draft the constitution from
    /// the guided answers plus an optional bounded own-words note. The host
    /// performs the one-shot call, pushes the sanitized/bounded draft back into the wizard, and opens the
    /// ratification preview; on any failure it reports why and leaves the
    /// deterministic guided draft standing. Nothing is persisted by this
    /// event — saving still goes through the ratify keypress and
    /// [`SetupConstitutionCommitRequested`](Self::SetupConstitutionCommitRequested).
    SetupConstitutionModelDraftRequested {
        draft: crate::tui::setup::GuidedConstitutionDraft,
        freeform_note: Option<String>,
        locale: crate::localization::Locale,
    },
    /// Emitted by the fleet setup Review step (`m`) to ask the configured
    /// model to draft the agent profile the wizard describes. The host
    /// performs the one-shot call, pushes the sanitized/bounded draft back
    /// into the wizard, and opens the rendered-TOML preview; on failure it
    /// reports why and the manual authoring flow stands. Nothing is
    /// persisted by this event.
    FleetProfileModelDraftRequested {
        role: String,
        /// Target model for the worker: a concrete model id, or "inherit".
        model: String,
        /// Canonical provider id for a concrete cross-provider route pick, or
        /// `None` for `inherit` (#4093). Carried so the model-drafted profile
        /// keeps the picked provider instead of collapsing to an ambiguous,
        /// provider-scoped profile — the exact bug #4093 fixes.
        provider: Option<String>,
        /// Canonical reasoning tier selected by the wizard, or `None` for
        /// inherit (#4137). Carried with the async draft for the same reason
        /// as `provider`: the ratified profile must preserve the operator's
        /// explicit choice, not whatever the model echoed.
        reasoning_effort: Option<String>,
        locale: crate::localization::Locale,
    },
    /// Emitted by the `/fleet` roster view (`s` / Enter) to hand off to the
    /// setup wizard for authoring or overriding a roster member. The roster
    /// view itself never writes anything.
    FleetRosterOpenSetupRequested,
    /// Emitted by the fleet setup Review step after the user previewed a
    /// model-drafted profile and pressed the explicit ratify key. The host
    /// renders TOML deterministically from the validated draft and persists
    /// it atomically under `.codewhale/agents/`.
    FleetProfileDraftCommitRequested {
        draft: Box<crate::fleet::profile::FleetProfileDraft>,
    },
    /// Emitted by the setup Runtime Posture card after the user has previewed
    /// and confirmed an explicit preset/config diff.
    SetupRuntimePresetApplyRequested {
        preset: crate::tui::setup::SetupRuntimePreset,
        state: codewhale_config::SetupState,
        message: String,
    },
    /// Emitted by the setup Provider/Model readiness card to hand off to the
    /// existing provider manager instead of duplicating provider auth UI.
    SetupOpenProviderRequested,
    /// Emitted by the setup Provider/Model readiness card to hand off to the
    /// existing provider-qualified model route picker.
    SetupOpenModelRequested,
    /// Emitted by the setup Operate/Fleet readiness card to hand off to the
    /// existing Fleet setup wizard without writing Fleet config itself.
    SetupOpenFleetRequested,
    /// Emitted by the setup Hotbar card to hand off to the existing Hotbar
    /// setup wizard without rewriting bindings itself.
    SetupOpenHotbarRequested,
    /// Emitted by the setup Runtime Posture card to hand off to the existing
    /// work-mode picker.
    SetupOpenModeRequested,
    /// Emitted by the setup Runtime Posture card to hand off to the existing
    /// config view for approval/sandbox/network details.
    SetupOpenConfigRequested,
    /// Emitted by the `/hotbar` setup wizard when the user chooses "Disable
    /// Hotbar". The host persists `hotbar = []` and hides the panel.
    HotbarDisableRequested,
    /// Emitted by the live-transcript overlay while in backtrack preview
    /// mode (#133) when the user steps the highlighted user message with
    /// Left or Right. The handler advances `app.backtrack`, refreshes the
    /// overlay's `selected_idx`, and pins scroll near the new highlight.
    BacktrackStep {
        direction: crate::tui::backtrack::Direction,
    },
    /// Emitted by the live-transcript overlay when the user presses Enter
    /// in backtrack preview mode (#133). The handler calls
    /// `app.backtrack.confirm()`, trims `app.history`/`api_messages` to
    /// the selected user message, populates the composer with the
    /// dropped user text, and closes the overlay.
    BacktrackConfirm,
    /// Emitted by the live-transcript overlay when the user presses Esc
    /// in backtrack preview mode (#133). The handler resets
    /// `app.backtrack` and closes the overlay without trimming.
    BacktrackCancel,
    ContextMenuSelected {
        action: ContextMenuAction,
    },
    /// Emitted by the pager (`c` / `y`) to copy its body to the system
    /// clipboard. The host handler writes via `app.clipboard` and surfaces a
    /// status message — modal views cannot reach `app` directly. `label` is
    /// the noun shown in the success / failure status (e.g. "Pager content").
    CopyToClipboard {
        text: String,
        label: String,
    },
}

#[derive(Debug, Clone)]
pub enum ViewAction {
    None,
    Close,
    Emit(ViewEvent),
    EmitAndClose(ViewEvent),
}

pub trait ModalView: std::any::Any {
    fn kind(&self) -> ModalKind;
    fn handle_key(&mut self, key: KeyEvent) -> ViewAction;
    /// Returns `true` if the modal consumed the paste; `false` to let the
    /// host route the text elsewhere (e.g. drop it because a modal is open,
    /// or insert it into the composer when no modal wants it). The default
    /// is `false` so modals that don't care about paste don't silently
    /// swallow Cmd-V.
    fn handle_paste(&mut self, _text: &str) -> bool {
        false
    }
    fn handle_mouse(&mut self, _mouse: MouseEvent) -> ViewAction {
        ViewAction::None
    }
    fn render(&self, area: Rect, buf: &mut Buffer);
    /// The region this modal actually paints within the full frame `area`.
    ///
    /// Defaults to the whole frame, which is the legacy full-screen overlay
    /// behaviour every picker/menu still relies on. Inline modals (the
    /// approval prompt) override this to return a bottom-anchored band so the
    /// backdrop only dims their strip and the transcript above stays visible.
    /// The returned rect MUST match the region the modal renders into, or the
    /// dim and the painted content will disagree.
    fn occupied_region(&self, area: Rect) -> Rect {
        area
    }
    fn update_subagents(&mut self, _agents: &[SubAgentResult]) -> bool {
        false
    }
    fn tick(&mut self) -> ViewAction {
        ViewAction::None
    }
    /// Erased downcast hook for views that need a typed reference back from
    /// the boxed trait object (e.g. the live transcript overlay needs `&mut`
    /// access from outside the trait so it can refresh its snapshot of the
    /// app's transcript state right before render).
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

#[derive(Default)]
pub struct ViewStack {
    views: Vec<Box<dyn ModalView>>,
}

impl ViewStack {
    pub fn new() -> Self {
        Self { views: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.views.is_empty()
    }

    pub fn top_kind(&self) -> Option<ModalKind> {
        self.views.last().map(|view| view.kind())
    }

    pub fn push<V: ModalView + 'static>(&mut self, view: V) {
        let kind = view.kind();
        self.views.push(Box::new(view));
        tracing::debug!(target: "codewhale_tui::view_stack", action = "push", kind = ?kind, depth = self.views.len(), "view pushed");
    }

    /// Push an already-boxed view back onto the stack. Used by call sites
    /// that pop a view, mutate it externally, and need to restore it without
    /// the generic `push` re-boxing dance.
    pub fn push_boxed(&mut self, view: Box<dyn ModalView>) {
        let kind = view.kind();
        self.views.push(view);
        tracing::debug!(target: "codewhale_tui::view_stack", action = "push_boxed", kind = ?kind, depth = self.views.len(), "view pushed");
    }

    pub fn pop(&mut self) -> Option<Box<dyn ModalView>> {
        let popped = self.views.pop();
        if let Some(view) = popped.as_ref() {
            tracing::debug!(target: "codewhale_tui::view_stack", action = "pop", kind = ?view.kind(), depth = self.views.len(), "view popped");
        }
        popped
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        // Dim each view's own occupied region rather than the whole frame, so
        // an inline modal (the approval prompt) leaves the transcript above it
        // visible instead of blacking out the screen. Full-screen modals keep
        // the default `occupied_region` of the entire frame, so their backdrop
        // is unchanged.
        for view in &self.views {
            let region = view.occupied_region(area);
            render_modal_backdrop(region, buf);
            view.render(area, buf);
        }
    }

    pub fn update_subagents(&mut self, agents: &[SubAgentResult]) -> bool {
        self.views
            .last_mut()
            .map(|view| view.update_subagents(agents))
            .unwrap_or(false)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Vec<ViewEvent> {
        let action = self
            .views
            .last_mut()
            .map(|view| view.handle_key(key))
            .unwrap_or(ViewAction::None);
        self.apply_action(action)
    }

    pub fn handle_paste(&mut self, text: &str) -> bool {
        self.views
            .last_mut()
            .map(|view| view.handle_paste(text))
            .unwrap_or(false)
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> Vec<ViewEvent> {
        let action = self
            .views
            .last_mut()
            .map(|view| view.handle_mouse(mouse))
            .unwrap_or(ViewAction::None);
        self.apply_action(action)
    }

    pub fn tick(&mut self) -> Vec<ViewEvent> {
        let action = self
            .views
            .last_mut()
            .map(|view| view.tick())
            .unwrap_or(ViewAction::None);
        self.apply_action(action)
    }

    fn apply_action(&mut self, action: ViewAction) -> Vec<ViewEvent> {
        let mut events = Vec::new();
        match action {
            ViewAction::None => {}
            ViewAction::Close => {
                if let Some(view) = self.views.pop() {
                    tracing::debug!(target: "codewhale_tui::view_stack", action = "close", kind = ?view.kind(), depth = self.views.len(), "view closed via action");
                }
            }
            ViewAction::Emit(event) => {
                events.push(event);
            }
            ViewAction::EmitAndClose(event) => {
                events.push(event);
                if let Some(view) = self.views.pop() {
                    tracing::debug!(target: "codewhale_tui::view_stack", action = "emit_and_close", kind = ?view.kind(), depth = self.views.len(), "view closed via action");
                }
            }
        }
        events
    }
}

impl fmt::Debug for ViewStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ViewStack")
            .field("len", &self.views.len())
            .field("top", &self.top_kind())
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigScope {
    Session,
    Saved,
}

impl ConfigScope {
    fn label(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                ConfigScope::Session => MessageId::ConfigScopeSession,
                ConfigScope::Saved => MessageId::ConfigScopeSaved,
            },
        )
    }

    fn persist(self) -> bool {
        matches!(self, ConfigScope::Saved)
    }
}

#[derive(Debug, Clone)]
struct ConfigRow {
    section: ConfigSection,
    key: String,
    value: String,
    editable: bool,
    scope: ConfigScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigSection {
    Provider,
    Model,
    Permissions,
    Network,
    Display,
    Composer,
    Sidebar,
    History,
    Mcp,
    Fleet,
    Experimental,
}

impl ConfigSection {
    fn label(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                ConfigSection::Provider => MessageId::ConfigSectionProvider,
                ConfigSection::Model => MessageId::ConfigSectionModel,
                ConfigSection::Permissions => MessageId::ConfigSectionPermissions,
                ConfigSection::Network => MessageId::ConfigSectionNetwork,
                ConfigSection::Display => MessageId::ConfigSectionDisplay,
                ConfigSection::Composer => MessageId::ConfigSectionComposer,
                ConfigSection::Sidebar => MessageId::ConfigSectionSidebar,
                ConfigSection::History => MessageId::ConfigSectionHistory,
                ConfigSection::Mcp => MessageId::ConfigSectionMcp,
                ConfigSection::Fleet => MessageId::ConfigSectionFleet,
                ConfigSection::Experimental => MessageId::ConfigSectionExperimental,
            },
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigListItem {
    Section(ConfigSection),
    Row(usize),
}

#[derive(Debug, Clone)]
struct ConfigEdit {
    key: String,
    original_value: String,
    buffer: Vec<char>,
    cursor: usize,
    select_all: bool,
    scope: ConfigScope,
}

pub struct ConfigView {
    rows: Vec<ConfigRow>,
    selected: usize,
    scroll: usize,
    editing: Option<ConfigEdit>,
    filter: String,
    status: Option<String>,
    locale: Locale,
    effective_cost_currency: String,
    last_visible_rows: Cell<usize>,
    last_row_hitboxes: RefCell<Vec<(u16, usize)>>,
}

const CONFIG_MIN_KEY_COLUMN_WIDTH: usize = 19;
const CONFIG_VALUE_COLUMN_WIDTH: usize = 44;
const CONFIG_MIN_VALUE_COLUMN_WIDTH: usize = 10;
const CONFIG_SCOPE_COLUMN_WIDTH: usize = 7;
const CONFIG_ROW_PREFIX_WIDTH: usize = 2;
const CONFIG_COLUMN_GAPS_WIDTH: usize = 2;

impl ConfigView {
    pub fn new_for_app(app: &App) -> Self {
        let settings = Settings::load().unwrap_or_else(|_| Settings::default());
        let config = Config::load(app.config_path.clone(), app.config_profile.as_deref())
            .unwrap_or_default();
        let mut rows = vec![
            ConfigRow {
                section: ConfigSection::Provider,
                key: "provider".to_string(),
                value: app.api_provider.as_str().to_string(),
                editable: true,
                scope: ConfigScope::Session,
            },
            ConfigRow {
                section: ConfigSection::Provider,
                key: config_base_url_row_key(app.api_provider).to_string(),
                value: config_base_url_row_value(app),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Model,
                key: "model".to_string(),
                value: app.model.clone(),
                editable: true,
                scope: ConfigScope::Session,
            },
            ConfigRow {
                section: ConfigSection::Model,
                key: "default_model".to_string(),
                value: settings
                    .default_model
                    .as_deref()
                    .unwrap_or(&*tr(app.ui_locale, MessageId::ConfigDefaultValue))
                    .to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Model,
                key: "reasoning_effort".to_string(),
                value: settings.reasoning_effort.as_deref().map_or_else(
                    || tr(app.ui_locale, MessageId::ConfigDefaultReasoning).to_string(),
                    |value| {
                        crate::tui::app::ReasoningEffort::from_setting_for_provider(
                            value,
                            app.api_provider,
                        )
                        .as_setting_for_provider(app.api_provider)
                        .to_string()
                    },
                ),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Permissions,
                key: "approval_mode".to_string(),
                value: app.approval_mode.label().to_string(),
                editable: true,
                scope: ConfigScope::Session,
            },
            ConfigRow {
                section: ConfigSection::Permissions,
                key: "default_mode".to_string(),
                value: settings.default_mode.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Permissions,
                key: "allow_shell".to_string(),
                value: app.allow_shell.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Network,
                key: "stream_chunk_timeout_secs".to_string(),
                value: app.stream_chunk_timeout_secs.to_string(),
                editable: true,
                scope: ConfigScope::Session,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "theme".to_string(),
                value: settings.theme.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "locale".to_string(),
                value: settings.locale.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "background_color".to_string(),
                value: settings.background_color.clone().unwrap_or_else(|| {
                    tr(app.ui_locale, MessageId::ConfigDefaultValue).to_string()
                }),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "calm_mode".to_string(),
                value: settings.calm_mode.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "low_motion".to_string(),
                value: settings.low_motion.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "fancy_animations".to_string(),
                value: settings.fancy_animations.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "show_thinking".to_string(),
                value: settings.show_thinking.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "show_tool_details".to_string(),
                value: settings.show_tool_details.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "status_indicator".to_string(),
                value: settings.status_indicator.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "synchronized_output".to_string(),
                value: settings.synchronized_output.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "cost_currency".to_string(),
                value: settings.cost_currency.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "transcript_spacing".to_string(),
                value: settings.transcript_spacing.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "tool_collapse".to_string(),
                value: settings.tool_collapse_mode.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "composer_density".to_string(),
                value: settings.composer_density.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "composer_border".to_string(),
                value: settings.composer_border.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "composer_vim_mode".to_string(),
                value: settings.composer_vim_mode.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "bracketed_paste".to_string(),
                value: settings.bracketed_paste.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "paste_burst_detection".to_string(),
                value: settings.paste_burst_detection.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "mention_menu_limit".to_string(),
                value: settings.mention_menu_limit.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "mention_menu_behavior".to_string(),
                value: settings.mention_menu_behavior.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "mention_walk_depth".to_string(),
                value: settings.mention_walk_depth.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Composer,
                key: "workspace_follow_symlinks".to_string(),
                value: settings.workspace_follow_symlinks.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Sidebar,
                key: "sidebar_width".to_string(),
                value: settings.sidebar_width_percent.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Sidebar,
                key: "sidebar_focus".to_string(),
                value: settings.sidebar_focus.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Sidebar,
                key: "context_panel".to_string(),
                value: settings.context_panel.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::History,
                key: "auto_compact".to_string(),
                value: settings.auto_compact.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::History,
                key: "auto_compact_threshold_percent".to_string(),
                value: format!("{:.0}", settings.auto_compact_threshold_percent),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::History,
                key: "max_history".to_string(),
                value: settings.max_input_history.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Mcp,
                key: "prefer_external_pdftotext".to_string(),
                value: settings.prefer_external_pdftotext.to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Mcp,
                key: "mcp_config_path".to_string(),
                value: app.mcp_config_path.display().to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Fleet,
                key: "fleet.exec.max_spawn_depth".to_string(),
                value: config
                    .fleet
                    .as_ref()
                    .map(|fleet| fleet.exec.max_spawn_depth)
                    .unwrap_or_else(|| codewhale_config::FleetExecConfig::default().max_spawn_depth)
                    .to_string(),
                editable: false,
                scope: ConfigScope::Saved,
            },
        ];
        rows.extend(experimental_config_rows(&config));

        Self {
            rows,
            selected: 0,
            scroll: 0,
            editing: None,
            filter: String::new(),
            status: None,
            locale: app.ui_locale,
            effective_cost_currency: cost_currency_config_value(app),
            last_visible_rows: Cell::new(0),
            last_row_hitboxes: RefCell::new(Vec::new()),
        }
    }

    fn tr(&self, id: MessageId) -> Cow<'static, str> {
        tr(self.locale, id)
    }

    fn visible_rows_cached(&self) -> usize {
        let cached = self.last_visible_rows.get();
        if cached == 0 { 8 } else { cached }
    }

    fn row_matches_filter(&self, row: &ConfigRow) -> bool {
        let filter = self.filter.trim().to_lowercase();
        if filter.is_empty() {
            return true;
        }

        let section = row.section.label(self.locale).to_lowercase();
        let section_en = row.section.label(Locale::En).to_lowercase();
        let label = config_label_for_key(&row.key).to_lowercase();
        let key = row.key.to_lowercase();
        let value = self.row_display_value(row).to_lowercase();
        let scope = row.scope.label(self.locale).to_lowercase();
        let scope_en = row.scope.label(Locale::En).to_lowercase();
        let hint = config_hint_for_key(&row.key).to_lowercase();

        filter.split_whitespace().all(|term| {
            section.contains(term)
                || section_en.contains(term)
                || label.contains(term)
                || key.contains(term)
                || value.contains(term)
                || scope.contains(term)
                || scope_en.contains(term)
                || hint.contains(term)
        })
    }

    fn matching_row_indices(&self) -> Vec<usize> {
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| self.row_matches_filter(row).then_some(idx))
            .collect()
    }

    fn visible_items(&self) -> Vec<ConfigListItem> {
        let mut items = Vec::new();
        let mut current_section = None;

        for (idx, row) in self.rows.iter().enumerate() {
            if !self.row_matches_filter(row) {
                continue;
            }

            if current_section != Some(row.section) {
                current_section = Some(row.section);
                items.push(ConfigListItem::Section(row.section));
            }
            items.push(ConfigListItem::Row(idx));
        }

        items
    }

    fn key_column_width(&self) -> usize {
        self.rows
            .iter()
            .map(|row| config_label_for_key(&row.key).chars().count())
            .max()
            .unwrap_or(CONFIG_MIN_KEY_COLUMN_WIDTH)
            .max(CONFIG_MIN_KEY_COLUMN_WIDTH)
    }

    fn table_column_widths(&self, content_width: usize) -> (usize, usize, usize) {
        let fixed_width =
            CONFIG_ROW_PREFIX_WIDTH + CONFIG_COLUMN_GAPS_WIDTH + CONFIG_SCOPE_COLUMN_WIDTH;
        let key_value_width = content_width.saturating_sub(fixed_width);
        let desired_key_width = self.key_column_width();

        if key_value_width == 0 {
            return (0, 0, CONFIG_SCOPE_COLUMN_WIDTH);
        }

        let minimum_key_width = CONFIG_MIN_KEY_COLUMN_WIDTH.min(key_value_width);
        let key_width = desired_key_width
            .min(key_value_width.saturating_sub(CONFIG_MIN_VALUE_COLUMN_WIDTH))
            .max(minimum_key_width);
        let value_width = key_value_width
            .saturating_sub(key_width)
            .min(CONFIG_VALUE_COLUMN_WIDTH);

        (key_width, value_width, CONFIG_SCOPE_COLUMN_WIDTH)
    }

    fn selected_row_index(&self) -> Option<usize> {
        let selected = self.selected;
        self.matching_row_indices()
            .into_iter()
            .any(|idx| idx == selected)
            .then_some(selected)
    }

    fn selected_display_position(&self, items: &[ConfigListItem]) -> Option<usize> {
        items
            .iter()
            .position(|item| matches!(item, ConfigListItem::Row(idx) if *idx == self.selected))
    }

    fn sync_selection_to_filter(&mut self) {
        let matches = self.matching_row_indices();
        if matches.is_empty() {
            self.selected = 0;
            self.scroll = 0;
            return;
        }

        if !matches.contains(&self.selected) {
            self.selected = matches[0];
        }
    }

    fn update_filter(&mut self, update: impl FnOnce(&mut String)) {
        update(&mut self.filter);
        self.status = None;
        self.sync_selection_to_filter();
        self.adjust_scroll(self.visible_rows_cached());
    }

    fn adjust_scroll(&mut self, visible_rows: usize) {
        self.sync_selection_to_filter();

        let items = self.visible_items();
        if items.is_empty() {
            self.scroll = 0;
            return;
        }

        let visible_rows = visible_rows.max(1);
        let max_scroll = items.len().saturating_sub(visible_rows);
        self.scroll = self.scroll.min(max_scroll);

        let Some(selected_pos) = self.selected_display_position(&items) else {
            self.scroll = 0;
            return;
        };

        if selected_pos < self.scroll {
            self.scroll = selected_pos;
        }

        if selected_pos >= self.scroll + visible_rows {
            self.scroll = selected_pos.saturating_sub(visible_rows.saturating_sub(1));
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let matches = self.matching_row_indices();
        if matches.is_empty() {
            return;
        }

        let current = matches
            .iter()
            .position(|idx| *idx == self.selected)
            .unwrap_or(0);
        let max = matches.len().saturating_sub(1);
        let next = if delta.is_negative() {
            current.saturating_sub(delta.unsigned_abs())
        } else {
            (current + delta as usize).min(max)
        };

        self.selected = matches[next];
        let visible_rows = self.visible_rows_cached();
        self.adjust_scroll(visible_rows);
    }

    fn handle_editing_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => {
                self.editing = None;
                self.status = Some(self.tr(MessageId::ConfigEditCancelled).to_string());
                ViewAction::None
            }
            KeyCode::Enter => {
                let Some(edit) = self.editing.take() else {
                    return ViewAction::None;
                };
                let submitted = edit.buffer.iter().collect::<String>();
                let value = submitted.trim().to_string();
                ViewAction::Emit(ViewEvent::ConfigUpdated {
                    key: edit.key,
                    value,
                    persist: edit.scope.persist(),
                })
            }
            KeyCode::Backspace => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.select_all {
                        edit.buffer.clear();
                        edit.cursor = 0;
                        edit.select_all = false;
                    } else if edit.cursor > 0 {
                        edit.cursor = edit.cursor.saturating_sub(1);
                        edit.buffer.remove(edit.cursor);
                    }
                }
                ViewAction::None
            }
            KeyCode::Delete => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.select_all {
                        edit.buffer.clear();
                        edit.cursor = 0;
                        edit.select_all = false;
                    } else if edit.cursor < edit.buffer.len() {
                        edit.buffer.remove(edit.cursor);
                    }
                }
                ViewAction::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.buffer.clear();
                    edit.cursor = 0;
                    edit.select_all = false;
                }
                ViewAction::None
            }
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.cursor = edit.buffer.len();
                    edit.select_all = true;
                }
                ViewAction::None
            }
            KeyCode::Left => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.select_all {
                        edit.cursor = 0;
                        edit.select_all = false;
                    } else {
                        edit.cursor = edit.cursor.saturating_sub(1);
                    }
                }
                ViewAction::None
            }
            KeyCode::Right => {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.select_all {
                        edit.cursor = edit.buffer.len();
                        edit.select_all = false;
                    } else {
                        edit.cursor = (edit.cursor + 1).min(edit.buffer.len());
                    }
                }
                ViewAction::None
            }
            KeyCode::Home => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.cursor = 0;
                    edit.select_all = false;
                }
                ViewAction::None
            }
            KeyCode::End => {
                if let Some(edit) = self.editing.as_mut() {
                    edit.cursor = edit.buffer.len();
                    edit.select_all = false;
                }
                ViewAction::None
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && !ch.is_control() =>
            {
                if let Some(edit) = self.editing.as_mut() {
                    if edit.select_all {
                        edit.buffer.clear();
                        edit.cursor = 0;
                        edit.select_all = false;
                    }
                    edit.buffer.insert(edit.cursor, ch);
                    edit.cursor += 1;
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn start_edit(&mut self) {
        let Some(row_idx) = self.selected_row_index() else {
            return;
        };
        let Some(row) = self.rows.get(row_idx) else {
            return;
        };
        let key = row.key.clone();
        let original_value = row.value.clone();
        let initial_value = match config_default_placeholder_message(&key) {
            Some(message_id)
                if original_value == tr(self.locale, message_id)
                    || original_value == tr(Locale::En, message_id) =>
            {
                String::new()
            }
            _ => original_value.clone(),
        };

        let buffer: Vec<char> = initial_value.chars().collect();
        self.editing = Some(ConfigEdit {
            key,
            original_value,
            cursor: buffer.len(),
            buffer,
            select_all: true,
            scope: row.scope,
        });
        self.status = None;
    }

    fn clear_filter(&mut self) {
        if self.filter.is_empty() {
            return;
        }

        self.update_filter(|filter| filter.clear());
    }

    fn row_display_value(&self, row: &ConfigRow) -> String {
        if row.key == "cost_currency" && row.scope == ConfigScope::Saved {
            let saved_cost_currency = crate::pricing::CostCurrency::from_setting(&row.value);
            let effective_cost_currency =
                crate::pricing::CostCurrency::from_setting(&self.effective_cost_currency);
            if saved_cost_currency != effective_cost_currency {
                return format!(
                    "{}{}",
                    row.value,
                    self.tr(MessageId::ConfigRowEffective)
                        .replace("{currency}", &self.effective_cost_currency)
                );
            }
        }

        row.value.clone()
    }

    fn selected_row_hint(&self) -> Option<String> {
        let row_idx = self.selected_row_index()?;
        let row = self.rows.get(row_idx)?;
        let label = config_label_for_key(&row.key);
        let hint = config_hint_for_key(&row.key);
        if !hint.is_empty() {
            return Some(format!("{label}: {hint}"));
        }
        if row.editable {
            Some(format!("{label}: Enter to edit ({})", row.key))
        } else {
            Some(format!("{label}: read-only status ({})", row.key))
        }
    }
}

fn config_base_url_row_key(provider: ApiProvider) -> &'static str {
    if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
        "base_url"
    } else {
        "provider_url"
    }
}

fn config_base_url_row_value(app: &App) -> String {
    Config::load(app.config_path.clone(), app.config_profile.as_deref())
        .map(|mut config| {
            config.provider = Some(app.api_provider.as_str().to_string());
            config.deepseek_base_url()
        })
        .unwrap_or_else(|_| tr(app.ui_locale, MessageId::ConfigUnavailable).to_string())
}

fn cost_currency_config_value(app: &App) -> String {
    match app.cost_currency {
        crate::pricing::CostCurrency::Usd => "usd",
        crate::pricing::CostCurrency::Cny => "cny",
    }
    .to_string()
}

fn experimental_config_rows(config: &Config) -> Vec<ConfigRow> {
    let features = config.features();
    let configured = config.features.as_ref().map(|table| &table.entries);
    let mut rows = Vec::new();

    for spec in FEATURES
        .iter()
        .filter(|spec| matches!(spec.stage, Stage::Experimental | Stage::Beta))
    {
        let effective = features.enabled(spec.id);
        let configured_value = configured
            .and_then(|entries| entries.get(spec.key))
            .copied();
        rows.push(ConfigRow {
            section: ConfigSection::Experimental,
            key: format!("features.{}", spec.key),
            value: experimental_feature_value(
                effective,
                spec.default_enabled,
                configured_value.is_some(),
            ),
            editable: false,
            scope: ConfigScope::Saved,
        });
    }

    rows.push(ConfigRow {
        section: ConfigSection::Fleet,
        key: "goal_command".to_string(),
        value:
            "/goal sets session objectives with optional token budgets; state shows in Work context"
                .to_string(),
        editable: false,
        scope: ConfigScope::Saved,
    });
    rows.push(ConfigRow {
        section: ConfigSection::Fleet,
        key: "workflow".to_string(),
        value:
            "/workflow runs scripted fan-out/fan-in operations with run cards and cancel support"
                .to_string(),
        editable: false,
        scope: ConfigScope::Saved,
    });

    rows
}

fn experimental_feature_value(effective: bool, default_enabled: bool, configured: bool) -> String {
    let state = if effective { "enabled" } else { "disabled" };
    let default_state = if default_enabled {
        "enabled"
    } else {
        "disabled"
    };
    if configured {
        format!("{state} (configured; default {default_state})")
    } else {
        format!("{state} (default {default_state})")
    }
}

fn config_label_for_key(key: &str) -> String {
    let static_label = match key {
        "provider" => "Active provider",
        "base_url" => "DeepSeek API URL",
        "provider_url" => "Provider API URL",
        "model" => "Current model",
        "default_model" => "Default model",
        "reasoning_effort" => "Reasoning level",
        "approval_mode" => "Current approval mode",
        "default_mode" => "Startup mode",
        "allow_shell" => "Shell access",
        "stream_chunk_timeout_secs" => "Stream timeout",
        "theme" => "Theme",
        "locale" => "Language",
        "background_color" => "Background",
        "calm_mode" => "Calm mode",
        "low_motion" => "Low motion",
        "fancy_animations" => "Animations",
        "show_thinking" => "Show thinking",
        "show_tool_details" => "Tool detail level",
        "status_indicator" => "Status indicator",
        "synchronized_output" => "Output pacing",
        "cost_currency" => "Cost currency",
        "transcript_spacing" => "Transcript spacing",
        "tool_collapse" => "Tool cards",
        "composer_density" => "Composer density",
        "composer_border" => "Composer border",
        "composer_vim_mode" => "Composer Vim mode",
        "bracketed_paste" => "Bracketed paste",
        "paste_burst_detection" => "Paste detection",
        "mention_menu_limit" => "Mention menu limit",
        "mention_menu_behavior" => "Mention menu behavior",
        "mention_walk_depth" => "File mention depth",
        "workspace_follow_symlinks" => "Follow symlinks",
        "sidebar_width" => "Sidebar width",
        "sidebar_focus" => "Sidebar focus",
        "context_panel" => "Context panel",
        "auto_compact" => "Auto compact",
        "auto_compact_threshold_percent" => "Compact threshold",
        "max_history" => "Input history",
        "prefer_external_pdftotext" => "PDF text extractor",
        "mcp_config_path" => "MCP config path",
        "fleet.exec.max_spawn_depth" => "Fleet recursion depth",
        "goal_command" => "Goal command",
        "workflow" => "Workflow",
        _ => {
            if let Some(feature) = key.strip_prefix("features.") {
                return format!("Feature: {}", humanize_config_key(feature));
            } else {
                return humanize_config_key(key);
            }
        }
    };
    static_label.to_string()
}

fn humanize_config_key(key: &str) -> String {
    key.split(['.', '_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut word = first.to_uppercase().collect::<String>();
            word.push_str(chars.as_str());
            word
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn config_hint_for_key(key: &str) -> &'static str {
    match key {
        "model" => "deepseek-v4-pro | deepseek-v4-flash | deepseek-*",
        "provider" => "deepseek | openrouter | xiaomi-mimo | fireworks | siliconflow | ...",
        "approval_mode" => "auto | suggest | never",
        "allow_shell" => "true enables shell in Act mode with approvals on the next turn",
        "auto_compact"
        | "calm_mode"
        | "low_motion"
        | "show_thinking"
        | "show_tool_details"
        | "composer_border"
        | "paste_burst_detection" => "on/off, true/false, yes/no, 1/0",
        "composer_density" | "transcript_spacing" => "compact | comfortable | spacious",
        "tool_collapse" => "compact | expanded | calm",
        "theme" => "system | dark | light | grayscale",
        "locale" => "auto | en | ja | zh-Hans | pt-BR",
        "background_color" => "#RRGGBB | default",
        "base_url" => "global DeepSeek/root fallback; e.g. https://api.deepseek.com/beta",
        "provider_url" => {
            "current provider endpoint; Xiaomi: token-plan | pay-as-you-go | custom URL"
        }
        "cost_currency" => "usd | cny",
        "default_mode" => "agent | plan | yolo",
        "sidebar_width" => "10..=50",
        "sidebar_focus" => "auto | work | tasks | agents | context | hidden",
        "max_history" => "integer (0 allowed)",
        "auto_compact_threshold_percent" => "10..=100",
        "default_model" => "deepseek-v4-pro | deepseek-v4-flash | deepseek-* | none/default",
        "reasoning_effort" => {
            "DeepSeek: auto/off/high/max; Codex: low/medium/high/xhigh; default clears saved value"
        }
        "mcp_config_path" => "path to mcp.json",
        "fleet.exec.max_spawn_depth" => {
            "0 blocks child agents; 3 default (same axis as sub-agents); capped at 8"
        }
        "features.subagents" => {
            "read-only feature flag state; /fleet setup is the user-facing path"
        }
        "features.web_search" => "read-only feature flag state for web search tools",
        "features.apply_patch" => "read-only feature flag state for patch editing tools",
        "features.mcp" => "read-only feature flag state for MCP tools",
        "features.exec_policy" => "read-only feature flag state for execution policy tools",
        "features.vision_model" => "beta feature flag for vision/model image support",
        "goal_command" => "/goal sets objectives, budgets, and Work-context status",
        "workflow" => "/workflow runs scripted operations with fan-out/fan-in run cards",
        _ => "",
    }
}

fn config_default_placeholder_message(key: &str) -> Option<MessageId> {
    match key {
        "default_model" | "background_color" => Some(MessageId::ConfigDefaultValue),
        "reasoning_effort" => Some(MessageId::ConfigDefaultReasoning),
        _ => None,
    }
}

fn render_config_editor_value_line(
    edit: &ConfigEdit,
    locale: Locale,
) -> ratatui::text::Line<'static> {
    use ratatui::{
        style::Style,
        text::{Line, Span},
    };

    let mut spans = Vec::new();
    spans.push(Span::styled(
        tr(locale, MessageId::ConfigEditNewLabel),
        Style::default().fg(palette::TEXT_MUTED),
    ));

    let cursor_style = Style::default()
        .fg(palette::WHALE_BG)
        .bg(palette::WHALE_INFO)
        .bold();
    let selected_style = Style::default()
        .fg(palette::SELECTION_TEXT)
        .bg(palette::SELECTION_BG);

    if edit.select_all && !edit.buffer.is_empty() {
        let text = edit.buffer.iter().collect::<String>();
        spans.push(Span::styled(text, selected_style));
        spans.push(Span::styled(" ", cursor_style));
        return Line::from(spans);
    }

    let before = edit.buffer.iter().take(edit.cursor).collect::<String>();
    spans.push(Span::raw(before));
    if edit.cursor < edit.buffer.len() {
        let ch = edit.buffer[edit.cursor];
        spans.push(Span::styled(ch.to_string(), cursor_style));
        let after = edit
            .buffer
            .iter()
            .skip(edit.cursor.saturating_add(1))
            .collect::<String>();
        spans.push(Span::raw(after));
    } else {
        spans.push(Span::styled(" ", cursor_style));
    }

    Line::from(spans)
}

impl ModalView for ConfigView {
    fn kind(&self) -> ModalKind {
        ModalKind::Config
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if self.editing.is_some() {
            return self.handle_editing_key(key);
        }

        match key.code {
            KeyCode::Esc => {
                if self.filter.is_empty() {
                    ViewAction::Close
                } else {
                    self.clear_filter();
                    ViewAction::None
                }
            }
            KeyCode::Char('q') if self.filter.is_empty() => ViewAction::Close,
            KeyCode::Up => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Char('k') if self.filter.is_empty() => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::Char('j') if self.filter.is_empty() => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.move_selection(-5);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.move_selection(5);
                ViewAction::None
            }
            KeyCode::Backspace => {
                if !self.filter.is_empty() {
                    self.update_filter(|filter| {
                        filter.pop();
                    });
                }
                ViewAction::None
            }
            // Ctrl+H is the legacy ASCII backspace many terminals emit.
            KeyCode::Char('h')
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                if !self.filter.is_empty() {
                    self.update_filter(|filter| {
                        filter.pop();
                    });
                }
                ViewAction::None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_filter();
                ViewAction::None
            }
            KeyCode::Char('e') | KeyCode::Char('E') if self.filter.is_empty() => {
                if self
                    .selected_row_index()
                    .and_then(|idx| self.rows.get(idx))
                    .is_some_and(|row| row.editable)
                {
                    self.start_edit();
                }
                ViewAction::None
            }
            KeyCode::Enter => {
                if self
                    .selected_row_index()
                    .and_then(|idx| self.rows.get(idx))
                    .is_some_and(|row| row.editable)
                {
                    self.start_edit();
                }
                ViewAction::None
            }
            KeyCode::Char(ch)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && !ch.is_control() =>
            {
                self.update_filter(|filter| filter.push(ch));
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if self.editing.is_some() {
            return ViewAction::None;
        }
        if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            return ViewAction::None;
        }

        let selected = self
            .last_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(y, row_idx)| (*y == mouse.row).then_some(*row_idx));
        if let Some(row_idx) = selected {
            self.selected = row_idx;
            self.status = None;
            self.adjust_scroll(self.visible_rows_cached());
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        use ratatui::{
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Padding, Paragraph, Widget},
        };

        let popup_area = centered_modal_area(area, 84, 22, 60, 12);

        render_modal_surface(area, popup_area, buf);

        let base_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = base_block.inner(popup_area);
        let (lines, footer) = if let Some(edit) = self.editing.as_ref() {
            let mut lines: Vec<Line> = Vec::new();
            let edit_label = config_label_for_key(&edit.key);
            let edit_title = if edit_label == edit.key {
                format!("{}{}", self.tr(MessageId::ConfigEditTitlePrefix), edit.key)
            } else {
                format!(
                    "{}{} [{}]",
                    self.tr(MessageId::ConfigEditTitlePrefix),
                    edit_label,
                    edit.key
                )
            };
            lines.push(Line::from(vec![Span::styled(
                edit_title,
                Style::default().fg(palette::WHALE_INFO).bold(),
            )]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    self.tr(MessageId::ConfigEditScopeLabel),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::raw(edit.scope.label(self.locale)),
            ]));
            lines.push(Line::from(vec![
                Span::styled(
                    self.tr(MessageId::ConfigEditCurrentLabel),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::raw(truncate_view_text(&edit.original_value, 60)),
            ]));
            lines.push(Line::from(""));
            lines.push(render_config_editor_value_line(edit, self.locale));
            lines.push(Line::from(""));
            let hint = config_hint_for_key(&edit.key);
            if !hint.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled(
                        self.tr(MessageId::ConfigEditHintLabel),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                    Span::raw(hint),
                ]));
            }
            (lines, self.tr(MessageId::ConfigEditFooter).to_string())
        } else {
            let content_height = usize::from(inner.height);
            // Title (with job subtitle), search, blank, column headers, separator.
            let header_lines = 5usize;
            let bottom_lines = 1usize;
            // The action footer now lives inside the modal body (reserved by
            // `render_modal_text_footer` below) rather than on the border, so it
            // claims one inner row that the table must not draw over.
            let footer_lines = 1usize;
            let visible_rows = content_height
                .saturating_sub(header_lines + bottom_lines + footer_lines)
                .max(1);
            self.last_visible_rows.set(visible_rows);

            let items = self.visible_items();
            let match_count = self.matching_row_indices().len();
            let start = self.scroll.min(items.len());
            let end = (start + visible_rows).min(items.len());
            let scrollable = items.len() > visible_rows;
            let search_value = if self.filter.is_empty() {
                self.tr(MessageId::ConfigSearchPlaceholder).to_string()
            } else {
                self.filter.clone()
            };

            let (key_column_width, value_column_width, scope_column_width) =
                self.table_column_widths(usize::from(inner.width));
            let mut lines: Vec<Line> = vec![
                Line::from(vec![
                    Span::styled(
                        self.tr(MessageId::ConfigTitle),
                        Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
                    ),
                    Span::styled(
                        format!(" — {}", self.tr(MessageId::ConfigSubtitle)),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("  Search: ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw(search_value),
                    Span::styled(
                        format!("  ({match_count}/{})", self.rows.len()),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]),
                Line::from(""),
                Line::from(format!(
                    "  {:<key_width$} {:<value_width$} {:<scope_width$}",
                    "Setting",
                    "Value",
                    "Scope",
                    key_width = key_column_width,
                    value_width = value_column_width,
                    scope_width = scope_column_width
                )),
                Line::from(format!(
                    "  {}",
                    "-".repeat(
                        key_column_width
                            + value_column_width
                            + scope_column_width
                            + CONFIG_COLUMN_GAPS_WIDTH
                    )
                )),
            ];
            let mut row_hitboxes = Vec::new();

            for item in items.iter().skip(start).take(visible_rows) {
                match item {
                    ConfigListItem::Section(section) => {
                        lines.push(Line::from(Span::styled(
                            format!("  {}", section.label(self.locale)),
                            Style::default().fg(palette::WHALE_INFO).bold(),
                        )));
                    }
                    ConfigListItem::Row(idx) => {
                        let Some(row) = self.rows.get(*idx) else {
                            continue;
                        };
                        let line_y = inner.y.saturating_add(lines.len() as u16);
                        row_hitboxes.push((line_y, *idx));
                        let selected = *idx == self.selected;
                        let style = if selected {
                            Style::default()
                                .fg(palette::SELECTION_TEXT)
                                .bg(palette::SELECTION_BG)
                                .add_modifier(ratatui::style::Modifier::BOLD)
                        } else {
                            Style::default().fg(palette::TEXT_PRIMARY)
                        };
                        let label = config_label_for_key(&row.key);
                        let key = truncate_view_text(&label, key_column_width);
                        let value =
                            truncate_view_text(&self.row_display_value(row), value_column_width);
                        let scope =
                            truncate_view_text(&row.scope.label(self.locale), scope_column_width);
                        let mut line = Line::from(format!(
                            "  {key:<key_column_width$} {value:<value_column_width$} {scope:<scope_column_width$}"
                        ));
                        line.style = style;
                        lines.push(line);
                    }
                }
            }
            *self.last_row_hitboxes.borrow_mut() = row_hitboxes;

            if items.is_empty() {
                let message = if self.filter.is_empty() {
                    self.tr(MessageId::ConfigNoSettings).to_string()
                } else {
                    format!(
                        "{}\"{}\".",
                        self.tr(MessageId::ConfigNoMatchesPrefix),
                        self.filter
                    )
                };
                lines.push(Line::from(Span::styled(
                    message,
                    Style::default().fg(palette::TEXT_MUTED),
                )));
            }

            let selected_hint = self.selected_row_hint();
            let bottom_text = if let Some(status) = self.status.as_ref() {
                status.clone()
            } else if !self.filter.is_empty() {
                format!(
                    "{}: {match_count}",
                    self.tr(MessageId::ConfigFilteredSettings)
                )
            } else if scrollable && !items.is_empty() {
                let showing = format!(
                    "{} {}-{} / {}",
                    self.tr(MessageId::ConfigShowing),
                    self.scroll.saturating_add(1),
                    end,
                    items.len()
                );
                if let Some(hint) = selected_hint {
                    format!("{showing} | {hint}")
                } else {
                    showing
                }
            } else {
                selected_hint.unwrap_or_default()
            };
            lines.push(Line::from(Span::styled(
                crate::tui::ui_text::semantic_truncate(&bottom_text, usize::from(inner.width)),
                Style::default().fg(palette::TEXT_MUTED),
            )));

            let footer = if !self.filter.is_empty() {
                self.tr(MessageId::ConfigFooterFiltered)
            } else if scrollable {
                self.tr(MessageId::ConfigFooterScrollable)
            } else {
                self.tr(MessageId::ConfigFooterDefault)
            };
            (lines, footer.to_string())
        };

        let block = Block::default()
            .title(Line::from(vec![Span::styled(
                self.tr(MessageId::ConfigModalTitle),
                Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
            )]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);
        // Footer wraps inside the body so its hints can never run off the modal
        // edge (#3732); the table renders into the area above it.
        let content = render_modal_text_footer(
            inner,
            buf,
            &footer,
            Style::default().fg(palette::TEXT_MUTED),
        );
        Paragraph::new(lines)
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .scroll((0, 0))
            .render(content, buf);
    }
}

pub mod help;

pub use help::HelpView;

pub struct SubAgentsView {
    agents: Vec<SubAgentResult>,
    scroll: usize,
}

/// Build the agent rows shown by `/subagents`.
///
/// The engine manager is the durable source of truth, but live UI cards can
/// briefly be ahead of the manager-list refresh. Include those live rows so
/// the command does not say "no agents" while the footer/sidebar already show
/// active delegated work.
pub(crate) fn subagent_view_agents(
    app: &App,
    manager_agents: &[SubAgentResult],
) -> Vec<SubAgentResult> {
    let mut agents = manager_agents.to_vec();
    let mut seen: std::collections::HashSet<String> =
        agents.iter().map(|agent| agent.agent_id.clone()).collect();

    for (agent_id, progress) in &app.agent_progress {
        if seen.insert(agent_id.clone()) {
            agents.push(live_subagent_result(
                agent_id,
                SubAgentType::General,
                SubAgentStatus::Running,
                progress,
                Some("live"),
                None, // live rows compute nickname from agent manager on render
            ));
        }
    }

    for cell in &app.history {
        match cell {
            HistoryCell::SubAgent(SubAgentCell::Delegate(card))
                if seen.insert(card.agent_id.clone()) =>
            {
                let agent_type =
                    SubAgentType::from_str(&card.agent_type).unwrap_or(SubAgentType::General);
                agents.push(live_subagent_result(
                    &card.agent_id,
                    agent_type,
                    lifecycle_to_subagent_status(card.status),
                    card.summary.as_deref().unwrap_or(card.agent_type.as_str()),
                    Some("transcript"),
                    None, // transcript-derived rows get nickname from manager on render
                ));
            }
            HistoryCell::SubAgent(SubAgentCell::Fanout(card)) => {
                for worker in &card.workers {
                    if seen.insert(worker.agent_id.clone()) {
                        let objective = format!(
                            "{} worker {}",
                            summarize_tool_output(&card.kind),
                            summarize_tool_output(&worker.worker_id)
                        );
                        agents.push(live_subagent_result(
                            &worker.agent_id,
                            SubAgentType::General,
                            lifecycle_to_subagent_status(worker.status),
                            &objective,
                            Some(card.kind.as_str()),
                            None, // fanout worker rows get nickname from manager on render
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    agents
}

fn lifecycle_to_subagent_status(status: AgentLifecycle) -> SubAgentStatus {
    match status {
        AgentLifecycle::Pending | AgentLifecycle::Running => SubAgentStatus::Running,
        AgentLifecycle::Completed => SubAgentStatus::Completed,
        AgentLifecycle::Failed => SubAgentStatus::Failed("failed in transcript".to_string()),
        AgentLifecycle::Cancelled => SubAgentStatus::Cancelled,
        AgentLifecycle::Interrupted => {
            SubAgentStatus::Interrupted("interrupted in transcript".to_string())
        }
    }
}

fn live_subagent_result(
    agent_id: &str,
    agent_type: SubAgentType,
    status: SubAgentStatus,
    objective: &str,
    role: Option<&str>,
    nickname: Option<String>,
) -> SubAgentResult {
    SubAgentResult {
        name: agent_id.to_string(),
        agent_id: agent_id.to_string(),
        context_mode: "fresh".to_string(),
        fork_context: false,
        workspace: None,
        git_branch: None,
        agent_type,
        assignment: SubAgentAssignment {
            objective: summarize_tool_output(objective),
            role: role.map(str::to_string),
        },
        model: String::new(),
        nickname,
        status,
        worker_status: None,
        parent_run_id: None,
        spawn_depth: 0,
        result: None,
        steps_taken: 0,
        checkpoint: None,
        needs_input: None,
        duration_ms: 0,
        from_prior_session: false,
    }
}

impl SubAgentsView {
    pub fn new(agents: Vec<SubAgentResult>) -> Self {
        Self { agents, scroll: 0 }
    }
}

impl ModalView for SubAgentsView {
    fn kind(&self) -> ModalKind {
        ModalKind::SubAgents
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        use crossterm::event::KeyCode;

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => {
                ViewAction::Emit(ViewEvent::SubAgentsRefresh)
            }
            KeyCode::Char('f') | KeyCode::Char('F') => {
                ViewAction::Emit(ViewEvent::CommandPaletteSelected {
                    action: CommandPaletteAction::ExecuteCommand {
                        command: "/fleet".to_string(),
                    },
                })
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn update_subagents(&mut self, agents: &[SubAgentResult]) -> bool {
        self.agents = agents.to_vec();
        self.scroll = self.scroll.min(self.agents.len().saturating_sub(1));
        true
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        use ratatui::{
            layout::Alignment,
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Padding, Paragraph, Widget},
        };

        let popup_area = centered_modal_area(area, 78, 20, 56, 12);

        render_modal_surface(area, popup_area, buf);

        let mut lines: Vec<Line> = Vec::new();
        let content_width = popup_area.width.saturating_sub(4) as usize;

        if self.agents.is_empty() {
            lines.push(Line::from(Span::styled(
                "No Fleet workers running.",
                Style::default().fg(palette::TEXT_MUTED),
            )));
            lines.push(Line::from(Span::styled(
                "Use /fleet to configure role profiles and launch posture.",
                Style::default().fg(palette::TEXT_DIM),
            )));
        } else {
            let mut running = Vec::new();
            let mut completed = Vec::new();
            let mut interrupted = Vec::new();
            let mut failed = Vec::new();
            let mut cancelled = Vec::new();

            for agent in &self.agents {
                match agent.status {
                    SubAgentStatus::Running => running.push(agent),
                    SubAgentStatus::Completed => completed.push(agent),
                    SubAgentStatus::Interrupted(_) => interrupted.push(agent),
                    SubAgentStatus::Failed(_) => failed.push(agent),
                    SubAgentStatus::Cancelled => cancelled.push(agent),
                    SubAgentStatus::BudgetExhausted => failed.push(agent),
                }
            }

            let status_summary = [
                ("Running", running.len(), palette::STATUS_WARNING),
                ("Completed", completed.len(), palette::STATUS_SUCCESS),
                ("Interrupted", interrupted.len(), palette::STATUS_WARNING),
                ("Failed", failed.len(), palette::WHALE_ERROR),
                ("Cancelled", cancelled.len(), palette::TEXT_MUTED),
            ];

            lines.push(Line::from(Span::styled(
                "Fleet workers",
                Style::default().fg(palette::WHALE_INFO).bold(),
            )));
            lines.push(Line::from(Span::styled(
                "Sub-agent roles are Fleet worker roles.",
                Style::default().fg(palette::TEXT_DIM),
            )));

            let mut summary_parts = Vec::new();
            for (label, count, color) in status_summary {
                summary_parts.push(Line::from(Span::styled(
                    format!("{label}: {count}"),
                    Style::default().fg(color),
                )));
            }

            let mut summary = vec![Span::styled("  ", Style::default().fg(palette::TEXT_DIM))];
            for (idx, part) in summary_parts.into_iter().enumerate() {
                if idx > 0 {
                    summary.push(Span::raw("  ·  "));
                }
                summary.extend(part);
            }
            lines.push(Line::from(summary));
            lines.push(Line::from(Span::styled(
                "",
                Style::default().fg(palette::TEXT_DIM),
            )));

            running.sort_by(|a, b| {
                let order = agent_type_order(&a.agent_type).cmp(&agent_type_order(&b.agent_type));
                order.then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            completed.sort_by(|a, b| {
                let order = agent_type_order(&a.agent_type).cmp(&agent_type_order(&b.agent_type));
                order.then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            interrupted.sort_by(|a, b| {
                let order = agent_type_order(&a.agent_type).cmp(&agent_type_order(&b.agent_type));
                order.then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            failed.sort_by(|a, b| {
                let order = agent_type_order(&a.agent_type).cmp(&agent_type_order(&b.agent_type));
                order.then_with(|| a.agent_id.cmp(&b.agent_id))
            });
            cancelled.sort_by(|a, b| {
                let order = agent_type_order(&a.agent_type).cmp(&agent_type_order(&b.agent_type));
                order.then_with(|| a.agent_id.cmp(&b.agent_id))
            });

            append_subagent_group(
                &mut lines,
                "Running",
                palette::STATUS_WARNING.into(),
                &running,
                content_width,
            );
            append_subagent_group(
                &mut lines,
                "Completed",
                palette::STATUS_SUCCESS.into(),
                &completed,
                content_width,
            );
            append_subagent_group(
                &mut lines,
                "Interrupted",
                palette::STATUS_WARNING.into(),
                &interrupted,
                content_width,
            );
            append_subagent_group(
                &mut lines,
                "Failed",
                palette::WHALE_ERROR.into(),
                &failed,
                content_width,
            );
            append_subagent_group(
                &mut lines,
                "Cancelled",
                palette::TEXT_MUTED.into(),
                &cancelled,
                content_width,
            );
        }

        // Reserve one body row for the wrapping footer below.
        let total_lines = lines.len();
        let visible_lines = usize::from(popup_area.height).saturating_sub(5).max(1);
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = self.scroll.min(max_scroll);

        let scroll_indicator = if total_lines > visible_lines {
            format!(" [{}/{} ↑↓] ", scroll + 1, max_scroll + 1)
        } else {
            String::new()
        };

        let block = Block::default()
            .title(Line::from(vec![Span::styled(
                " Fleet workers ",
                Style::default().fg(palette::WHALE_ACCENT_PRIMARY).bold(),
            )]))
            .title_bottom(
                Line::from(Span::styled(
                    scroll_indicator,
                    Style::default().fg(palette::WHALE_INFO),
                ))
                .alignment(Alignment::Right),
            )
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("Esc", "close"),
                ActionHint::new("R", "refresh"),
                ActionHint::new("F", "setup"),
            ],
        );

        Paragraph::new(lines)
            .scroll((scroll as u16, 0))
            .render(content, buf);
    }
}

fn append_subagent_group(
    lines: &mut Vec<ratatui::text::Line<'static>>,
    title: &str,
    section_style: ratatui::style::Style,
    agents: &[&SubAgentResult],
    content_width: usize,
) {
    use ratatui::{
        style::Style,
        text::{Line, Span},
    };
    if agents.is_empty() {
        return;
    }

    lines.push(Line::from(Span::styled(
        format!("{title} ({})", agents.len()),
        section_style.bold(),
    )));

    for agent in agents {
        let id = truncate_view_text(&agent.agent_id, 11);
        let display_name = agent
            .nickname
            .as_deref()
            .map(|nick| format!("{nick:<12}"))
            .unwrap_or_else(|| format!("{id:<12}"));
        let kind = format_agent_type(&agent.agent_type);
        let (status, status_style, status_detail) = format_agent_status(&agent.status);

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(display_name, Style::default().fg(palette::TEXT_PRIMARY)),
            Span::raw(" "),
            Span::styled(format!("{id:<11}"), Style::default().fg(palette::TEXT_DIM)),
            Span::styled(
                format!("{kind:<9}"),
                Style::default().fg(palette::TEXT_MUTED),
            ),
            Span::raw("  "),
            Span::styled(format!("{status:<10}"), status_style),
            Span::raw("  "),
            Span::styled(
                format!("{:>4}✦", agent.steps_taken),
                Style::default().fg(palette::TEXT_DIM),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{:>6}ms", agent.duration_ms),
                Style::default().fg(palette::TEXT_DIM),
            ),
        ]));

        if let Some(detail) = status_detail {
            let max_len = content_width.saturating_sub(10);
            let detail = truncate_view_text(detail, max_len);
            lines.push(Line::from(vec![
                Span::styled("    reason: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(detail, Style::default().fg(palette::WHALE_ERROR)),
            ]));
        }

        if let Some(role) = agent.assignment.role.as_deref() {
            let max_len = content_width.saturating_sub(14);
            let role = truncate_view_text(role, max_len);
            lines.push(Line::from(vec![
                Span::styled("    role: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(role, Style::default().fg(palette::WHALE_INFO)),
            ]));
        }

        if let Some(branch) = agent.git_branch.as_deref() {
            let workspace = agent
                .workspace
                .as_deref()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty());
            let mut branch_detail = format!("branch {branch}");
            if let Some(workspace) = workspace {
                branch_detail.push_str(&format!(" @ {workspace}"));
            }
            let max_len = content_width.saturating_sub(14);
            let branch_detail = truncate_view_text(&branch_detail, max_len);
            lines.push(Line::from(vec![
                Span::styled("    git: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(branch_detail, Style::default().fg(palette::WHALE_INFO)),
            ]));
        }

        let max_len = content_width.saturating_sub(18);
        let objective = truncate_view_text(&agent.assignment.objective, max_len);
        lines.push(Line::from(vec![
            Span::styled("    objective: ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(objective, Style::default().fg(palette::TEXT_DIM)),
        ]));

        if let Some(result) = agent.result.as_ref() {
            let max_len = content_width.saturating_sub(16);
            let preview = truncate_view_text(result, max_len);
            lines.push(Line::from(vec![
                Span::styled("    result: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(preview, Style::default().fg(palette::TEXT_DIM)),
            ]));
        }
    }

    lines.push(Line::from(""));
}

fn agent_type_order(agent_type: &SubAgentType) -> u8 {
    match agent_type {
        SubAgentType::General => 0,
        SubAgentType::Explore => 1,
        SubAgentType::Plan => 2,
        SubAgentType::Implementer => 3,
        SubAgentType::Verifier => 4,
        SubAgentType::Review => 5,
        SubAgentType::Custom => 6,
    }
}

fn format_agent_type(agent_type: &SubAgentType) -> &'static str {
    // Source of truth lives on the enum so any new role lands in both
    // the user-visible label and the sort order via the as_str() helper.
    agent_type.as_str()
}

fn format_agent_status(
    status: &SubAgentStatus,
) -> (&'static str, ratatui::style::Style, Option<&str>) {
    use ratatui::style::Style;

    match status {
        SubAgentStatus::Running => ("running", Style::default().fg(palette::WHALE_INFO), None),
        SubAgentStatus::Completed => (
            "completed",
            Style::default().fg(palette::WHALE_ACCENT_PRIMARY),
            None,
        ),
        SubAgentStatus::Interrupted(reason) => (
            "interrupted",
            Style::default().fg(palette::STATUS_WARNING),
            Some(reason.as_str()),
        ),
        SubAgentStatus::Cancelled => ("cancelled", Style::default().fg(palette::TEXT_MUTED), None),
        SubAgentStatus::BudgetExhausted => (
            "budget_exhausted",
            Style::default().fg(palette::STATUS_WARNING),
            None,
        ),
        SubAgentStatus::Failed(reason) => (
            "failed",
            Style::default().fg(palette::WHALE_ERROR),
            Some(reason.as_str()),
        ),
    }
}

fn truncate_view_text(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => text[..idx].to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionHint, ConfigListItem, ConfigView, EmptyState, HelpView, ListDetailLayout, ModalKind,
        ModalView, ViewAction, ViewEvent, ViewStack, action_footer_lines, centered_modal_area,
        render_modal_footer, subagent_view_agents, truncate_view_text,
    };
    use crate::config::Config;
    use crate::localization::{Locale, MessageId, tr};
    use crate::palette;
    use crate::settings::Settings;
    use crate::tools::subagent::{
        SubAgentAssignment, SubAgentResult, SubAgentStatus, SubAgentType,
    };
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::history::{HistoryCell, SubAgentCell};
    use crate::tui::views::{CommandPaletteAction, SubAgentsView};
    use crate::tui::widgets::agent_card::{AgentLifecycle, FanoutCard};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{
        buffer::Buffer,
        layout::Rect,
        style::{Color, Style},
    };
    use std::borrow::Cow;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::MutexGuard;
    use tempfile::TempDir;
    use unicode_width::UnicodeWidthStr;

    /// Terminal sizes the v0.8.66 modal blocker (#3732) requires every overlay
    /// to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    /// Render a modal through the `ViewStack` (so the shared opaque backdrop is
    /// painted exactly as in production) over a sentinel-filled buffer, then
    /// assert: every `required_label` is visible, no sentinel `X` survives
    /// anywhere (fully opaque), the center cell carries the modal ink, and no
    /// row overflows the frame width.
    fn assert_modal_usable_and_opaque<V: ModalView + 'static>(
        make: impl Fn() -> V,
        required_labels: &[&str],
    ) {
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(make());
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in required_labels {
                assert!(text.contains(label), "{w}x{h}: missing '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::WHALE_BG,
                "{w}x{h}: modal interior must be opaque"
            );
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }

    #[test]
    fn config_modal_is_usable_and_opaque_at_blocker_sizes() {
        let _lock = crate::test_support::lock_test_env();
        // "Search" is the hardcoded English search-row label; asserting it (plus
        // the opacity/overflow checks) proves the modal renders fully and its
        // footer wraps inside bounds rather than clipping.
        assert_modal_usable_and_opaque(|| create_config_view(Locale::En), &["Search"]);
    }

    #[test]
    fn subagents_modal_is_usable_and_opaque_at_blocker_sizes() {
        assert_modal_usable_and_opaque(
            || SubAgentsView::new(Vec::new()),
            &["close", "refresh", "setup"],
        );
    }

    #[test]
    fn centered_modal_area_clamps_and_centers() {
        // Roomy frame: preferred size honoured, centered.
        let area = Rect::new(0, 0, 160, 40);
        let rect = centered_modal_area(area, 80, 20, 40, 10);
        assert_eq!((rect.width, rect.height), (80, 20));
        assert_eq!(rect.x, (160 - 80) / 2);
        assert_eq!(rect.y, (40 - 20) / 2);

        // Tiny frame: never exceeds the frame even below the requested minimum.
        let tiny = Rect::new(0, 0, 30, 8);
        let rect = centered_modal_area(tiny, 80, 20, 40, 10);
        assert!(rect.width <= tiny.width, "width must fit frame");
        assert!(rect.height <= tiny.height, "height must fit frame");
        assert!(rect.x + rect.width <= tiny.width);
        assert!(rect.y + rect.height <= tiny.height);
    }

    #[test]
    fn action_footer_wraps_instead_of_overflowing() {
        let hints = [
            ActionHint::new("↑↓", "move"),
            ActionHint::new("a-z", "jump"),
            ActionHint::new("Enter", "apply"),
            ActionHint::new("R", "edit key"),
            ActionHint::new("M", "models"),
            ActionHint::new("Esc", "cancel"),
        ];

        // Wide enough for a single row.
        let wide = action_footer_lines(&hints, 120);
        assert_eq!(wide.len(), 1);
        assert!(wide[0].width() <= 120);

        // Narrow forces wrapping but never truncates: every action survives and
        // no produced line exceeds the available width.
        let narrow = action_footer_lines(&hints, 28);
        assert!(narrow.len() >= 2, "narrow footer should wrap to >1 row");
        for line in &narrow {
            assert!(
                line.width() <= 28,
                "wrapped footer row overflows: {} cols",
                line.width()
            );
        }
        let joined: String = narrow
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect();
        for label in ["move", "jump", "apply", "edit key", "models", "cancel"] {
            assert!(joined.contains(label), "footer dropped action: {label}");
        }
    }

    #[test]
    fn render_modal_footer_reserves_rows_and_returns_body() {
        let inner = Rect::new(2, 2, 40, 10);
        let mut buf = Buffer::empty(Rect::new(0, 0, 44, 14));
        let hints = [
            ActionHint::new("Enter", "save"),
            ActionHint::new("Esc", "cancel"),
        ];
        let body = render_modal_footer(inner, &mut buf, &hints);
        // The footer (a single row at this width) is reserved off the bottom and
        // the body fills the rows above it.
        assert_eq!(body.y, inner.y);
        assert_eq!(body.height, inner.height - 1);
        assert_eq!(body.y + body.height, inner.y + inner.height - 1);
    }

    #[test]
    fn list_detail_layout_splits_wide_and_stacks_narrow() {
        let wide = ListDetailLayout::split(Rect::new(0, 0, 120, 24), 34);
        assert!(!wide.stacked);
        assert!(wide.list.width >= 30);
        assert!(wide.detail.width >= 34);
        assert_eq!(wide.list.height, 24);
        assert_eq!(wide.detail.height, 24);
        assert!(wide.list.right() < wide.detail.left());

        let narrow = ListDetailLayout::split(Rect::new(0, 0, 80, 20), 34);
        assert!(narrow.stacked);
        assert_eq!(narrow.list.width, 80);
        assert_eq!(narrow.detail.width, 80);
        assert!(narrow.list.bottom() <= narrow.detail.top());
        assert!(narrow.list.height > 0);
    }

    #[test]
    fn empty_state_renders_copy_and_actions() {
        let area = Rect::new(0, 0, 48, 8);
        let mut buf = Buffer::empty(area);
        EmptyState::new("Nothing here", "Use search or switch categories.")
            .primary_action("/", "filter")
            .secondary_action("Esc", "cancel")
            .render(area, &mut buf);

        let text = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in ["Nothing here", "Use search", "filter", "cancel"] {
            assert!(
                text.contains(expected),
                "empty state missing {expected:?}: {text:?}"
            );
        }
    }

    struct ConfigSettingsEnvGuard {
        _tmp: TempDir,
        previous_config_path: Option<OsString>,
        _lock: MutexGuard<'static, ()>,
    }

    impl ConfigSettingsEnvGuard {
        fn new(settings_toml: &str) -> Self {
            let lock = crate::test_support::lock_test_env();
            let tmp = TempDir::new().expect("settings tempdir");
            let config_path = tmp.path().join(".deepseek").join("config.toml");
            let settings_path = config_path
                .parent()
                .expect("settings parent")
                .join("settings.toml");
            std::fs::create_dir_all(config_path.parent().expect("config parent"))
                .expect("config dir");
            std::fs::write(&settings_path, settings_toml).expect("settings file");
            let previous_config_path = std::env::var_os("DEEPSEEK_CONFIG_PATH");
            unsafe {
                std::env::set_var("DEEPSEEK_CONFIG_PATH", &config_path);
            }
            Self {
                _tmp: tmp,
                previous_config_path,
                _lock: lock,
            }
        }
    }

    impl Drop for ConfigSettingsEnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.previous_config_path.take() {
                    Some(previous) => std::env::set_var("DEEPSEEK_CONFIG_PATH", previous),
                    None => std::env::remove_var("DEEPSEEK_CONFIG_PATH"),
                }
            }
        }
    }

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
        let mut app = App::new(options, &Config::default());
        app.api_provider = crate::config::ApiProvider::Deepseek;
        app
    }

    fn cost_currency_row_for_settings(
        settings_toml: &str,
    ) -> (String, String, crate::pricing::CostCurrency, Locale) {
        let _guard = ConfigSettingsEnvGuard::new(settings_toml);
        let app = create_test_app();
        let view = ConfigView::new_for_app(&app);
        let row = view
            .rows
            .iter()
            .find(|row| row.key == "cost_currency")
            .expect("cost_currency row");

        (
            row.value.clone(),
            view.row_display_value(row),
            app.cost_currency,
            app.ui_locale,
        )
    }

    fn type_filter(view: &mut ConfigView, text: &str) {
        for ch in text.chars() {
            let action = view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            assert!(matches!(action, ViewAction::None));
        }
    }

    fn manager_agent(id: &str, status: SubAgentStatus) -> SubAgentResult {
        SubAgentResult {
            name: id.to_string(),
            agent_id: id.to_string(),
            context_mode: "fresh".to_string(),
            fork_context: false,
            workspace: None,
            git_branch: None,
            agent_type: SubAgentType::Explore,
            assignment: SubAgentAssignment {
                objective: "read the docs".to_string(),
                role: None,
            },
            model: "deepseek-v4-flash".to_string(),
            nickname: None,
            status,
            worker_status: None,
            parent_run_id: None,
            spawn_depth: 0,
            result: None,
            steps_taken: 1,
            checkpoint: None,
            needs_input: None,
            duration_ms: 10,
            from_prior_session: false,
        }
    }

    #[test]
    fn subagent_view_agents_includes_progress_only_running_agent() {
        let mut app = create_test_app();
        app.agent_progress
            .insert("agent_live".to_string(), "reading code".to_string());

        let agents = subagent_view_agents(&app, &[]);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, "agent_live");
        assert!(matches!(agents[0].status, SubAgentStatus::Running));
        assert_eq!(agents[0].assignment.role.as_deref(), Some("live"));
        assert!(agents[0].assignment.objective.contains("reading code"));
    }

    #[test]
    fn subagent_view_agents_includes_live_fanout_workers_when_cache_is_empty() {
        let mut app = create_test_app();
        let mut card = FanoutCard::new("rlm").with_workers(["chunk_1", "chunk_2"]);
        card.upsert_worker("chunk_1", AgentLifecycle::Completed);
        card.upsert_worker("chunk_2", AgentLifecycle::Running);
        app.add_message(HistoryCell::SubAgent(SubAgentCell::Fanout(card)));
        app.last_fanout_card_index = Some(app.history.len().saturating_sub(1));

        let agents = subagent_view_agents(&app, &[]);

        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0].agent_id, "chunk_1");
        assert!(matches!(agents[0].status, SubAgentStatus::Completed));
        assert_eq!(agents[1].agent_id, "chunk_2");
        assert!(matches!(agents[1].status, SubAgentStatus::Running));
        assert_eq!(agents[1].assignment.role.as_deref(), Some("rlm"));
    }

    #[test]
    fn subagent_view_agents_deduplicates_manager_rows_over_live_rows() {
        let mut app = create_test_app();
        app.agent_progress
            .insert("agent_cached".to_string(), "live duplicate".to_string());
        let manager = vec![manager_agent("agent_cached", SubAgentStatus::Running)];

        let agents = subagent_view_agents(&app, &manager);

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_type, SubAgentType::Explore);
        assert_eq!(agents[0].assignment.objective, "read the docs");
    }

    #[test]
    fn fleet_worker_status_view_can_jump_to_fleet_setup() {
        let mut view = SubAgentsView::new(Vec::new());

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));

        match action {
            ViewAction::Emit(ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::ExecuteCommand { command },
            }) => assert_eq!(command, "/fleet"),
            other => panic!("expected /fleet jump action, got {other:?}"),
        }
    }

    fn visible_section_labels(view: &ConfigView) -> Vec<Cow<'static, str>> {
        view.visible_items()
            .into_iter()
            .filter_map(|item| match item {
                ConfigListItem::Section(section) => Some(section.label(view.locale)),
                ConfigListItem::Row(_) => None,
            })
            .collect()
    }

    fn create_config_view(locale: Locale) -> ConfigView {
        let mut app = create_test_app();
        app.ui_locale = locale;
        ConfigView::new_for_app(&app)
    }

    fn visible_row_keys(view: &ConfigView) -> Vec<&str> {
        view.visible_items()
            .into_iter()
            .filter_map(|item| match item {
                ConfigListItem::Row(idx) => Some(view.rows[idx].key.as_str()),
                ConfigListItem::Section(_) => None,
            })
            .collect()
    }

    #[test]
    fn truncate_view_text_handles_unicode() {
        let text = "abc😀é";
        assert_eq!(truncate_view_text(text, 0), "");
        assert_eq!(truncate_view_text(text, 1), "a");
        assert_eq!(truncate_view_text(text, 3), "abc");
        assert_eq!(truncate_view_text(text, 4), "abc😀");
        assert_eq!(truncate_view_text(text, 5), "abc😀é");
    }

    #[test]
    fn config_view_groups_rows_by_expected_sections() {
        let view = create_config_view(Locale::En);
        assert_eq!(
            visible_section_labels(&view),
            vec![
                "Provider",
                "Model",
                "Permissions",
                "Network",
                "Display",
                "Composer",
                "Sidebar",
                "History",
                "MCP",
                "Fleet",
                "Experimental",
                "Fleet",
            ]
        );
    }

    #[test]
    fn config_view_includes_expected_editable_rows() {
        let app = create_test_app();
        let view = ConfigView::new_for_app(&app);
        let keys = view
            .rows
            .iter()
            .map(|row| row.key.as_str())
            .collect::<Vec<_>>();
        assert!(keys.contains(&"provider"));
        assert!(keys.contains(&"model"));
        assert!(keys.contains(&"reasoning_effort"));
        assert!(keys.contains(&"base_url"));
        assert!(keys.contains(&"approval_mode"));
        assert!(keys.contains(&"allow_shell"));
        assert!(keys.contains(&"stream_chunk_timeout_secs"));
        assert!(keys.contains(&"theme"));
        assert!(keys.contains(&"locale"));
        assert!(keys.contains(&"background_color"));
        assert!(keys.contains(&"fancy_animations"));
        assert!(keys.contains(&"status_indicator"));
        assert!(keys.contains(&"synchronized_output"));
        assert!(keys.contains(&"auto_compact"));
        assert!(keys.contains(&"tool_collapse"));
        assert!(keys.contains(&"composer_border"));
        assert!(keys.contains(&"composer_vim_mode"));
        assert!(keys.contains(&"bracketed_paste"));
        assert!(keys.contains(&"context_panel"));
        assert!(keys.contains(&"cost_currency"));
        assert!(keys.contains(&"prefer_external_pdftotext"));
        assert!(keys.contains(&"mcp_config_path"));
        assert!(keys.contains(&"fleet.exec.max_spawn_depth"));
        assert!(keys.contains(&"features.vision_model"));
        assert!(keys.contains(&"goal_command"));
        assert!(keys.contains(&"workflow"));
        assert!(!keys.contains(&"features.subagents"));
        assert!(!keys.contains(&"features.web_search"));
        assert!(!keys.contains(&"features.apply_patch"));
        assert!(!keys.contains(&"features.mcp"));
        assert!(!keys.contains(&"features.exec_policy"));
        assert!(!keys.contains(&"whaleflow"));
        assert!(
            view.rows
                .iter()
                .filter(|row| {
                    !matches!(
                        row.section,
                        super::ConfigSection::Experimental | super::ConfigSection::Fleet
                    )
                })
                .all(|row| row.editable)
        );
        assert!(
            view.rows
                .iter()
                .filter(|row| {
                    matches!(
                        row.section,
                        super::ConfigSection::Experimental | super::ConfigSection::Fleet
                    )
                })
                .all(|row| !row.editable)
        );
    }

    #[test]
    fn config_view_experimental_features_show_effective_state_and_overrides() {
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-experimental-config-view-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("config.toml");
        fs::write(
            &config_path,
            r#"
[features]
web_search = false
vision_model = true
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path);
        let view = ConfigView::new_for_app(&app);

        let web_search = view
            .rows
            .iter()
            .find(|row| row.key == "features.web_search");
        assert!(web_search.is_none());

        let vision = view
            .rows
            .iter()
            .find(|row| row.key == "features.vision_model")
            .expect("vision feature row");
        assert_eq!(vision.value, "enabled (configured; default disabled)");
        assert!(!vision.editable);

        let subagents = view.rows.iter().find(|row| row.key == "features.subagents");
        assert!(subagents.is_none());
    }

    #[test]
    fn config_view_shows_fleet_max_spawn_depth_from_config() {
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-fleet-config-view-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("config.toml");
        fs::write(
            &config_path,
            r#"
[fleet.exec]
max_spawn_depth = 2
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path);
        let view = ConfigView::new_for_app(&app);

        let row = view
            .rows
            .iter()
            .find(|row| row.key == "fleet.exec.max_spawn_depth")
            .expect("fleet spawn depth row");
        assert_eq!(row.value, "2");
        assert!(!row.editable);
    }

    #[test]
    fn config_view_experimental_section_is_searchable() {
        let mut view = create_config_view(Locale::En);

        view.update_filter(|filter| filter.push_str("experimental"));
        assert_eq!(visible_section_labels(&view), vec!["Experimental"]);
        assert_eq!(visible_row_keys(&view), vec!["features.vision_model"]);

        view.clear_filter();
        type_filter(&mut view, "feature vision");
        assert_eq!(visible_section_labels(&view), vec!["Experimental"]);
        assert_eq!(visible_row_keys(&view), vec!["features.vision_model"]);

        view.clear_filter();
        type_filter(&mut view, "goal");
        assert_eq!(visible_section_labels(&view), vec!["Fleet"]);
        assert_eq!(visible_row_keys(&view), vec!["goal_command"]);

        view.clear_filter();
        type_filter(&mut view, "workflow");
        assert_eq!(visible_section_labels(&view), vec!["Fleet"]);
        assert_eq!(visible_row_keys(&view), vec!["workflow"]);

        view.clear_filter();
        type_filter(&mut view, "whaleflow");
        assert!(visible_row_keys(&view).is_empty());
    }

    #[test]
    fn config_view_base_url_reflects_app_config_path() {
        let temp_root = std::env::temp_dir().join(format!(
            "deepseek-tui-base-url-view-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("config.toml");
        fs::write(
            &config_path,
            "base_url = \"https://ui-config-view.local/v1\"\n",
        )
        .unwrap();

        let mut app = create_test_app();
        app.config_path = Some(config_path.clone());
        let view = ConfigView::new_for_app(&app);

        let row = view
            .rows
            .iter()
            .find(|row| row.key == "base_url")
            .expect("base_url row missing");
        assert_eq!(row.value, "https://ui-config-view.local/v1");
    }

    #[test]
    fn config_view_uses_provider_url_for_non_deepseek_provider() {
        let temp_root = std::env::temp_dir().join(format!(
            "codewhale-provider-url-view-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&temp_root).unwrap();
        let config_path = temp_root.join("config.toml");
        fs::write(
            &config_path,
            r#"
provider = "xiaomi-mimo"

[providers.xiaomi_mimo]
api_key = "tp-test-token-plan-key"
base_url = "https://api.xiaomimimo.com/v1"
"#,
        )
        .unwrap();

        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::XiaomiMimo;
        app.config_path = Some(config_path.clone());
        let view = ConfigView::new_for_app(&app);

        let row = view
            .rows
            .iter()
            .find(|row| row.key == "provider_url")
            .expect("provider_url row missing");
        assert_eq!(row.value, crate::config::DEFAULT_XIAOMI_MIMO_BASE_URL);
        assert!(!view.rows.iter().any(|row| row.key == "base_url"));
    }

    #[test]
    fn config_view_cost_currency_shows_saved_and_effective_runtime_currency() {
        let _guard = ConfigSettingsEnvGuard::new("locale = \"zh-Hans\"\ncost_currency = \"usd\"\n");
        let app = create_test_app();
        assert_eq!(app.ui_locale, Locale::ZhHans);
        assert_eq!(app.cost_currency, crate::pricing::CostCurrency::Cny);

        let view = ConfigView::new_for_app(&app);
        let row = view
            .rows
            .iter()
            .find(|row| row.key == "cost_currency")
            .expect("cost_currency row");

        assert_eq!(row.value, "usd");
        assert_eq!(view.row_display_value(row), "usd (实际 cny)");
        assert_eq!(Settings::load().expect("settings").cost_currency, "usd");
    }

    #[test]
    fn config_view_cost_currency_aliases_matching_effective_currency_are_silent() {
        for alias in ["rmb", "yuan", "¥"] {
            let (saved_value, display_value, effective_currency, locale) =
                cost_currency_row_for_settings(&format!(
                    "locale = \"zh-Hans\"\ncost_currency = \"{alias}\"\n"
                ));

            assert_eq!(locale, Locale::ZhHans);
            assert_eq!(effective_currency, crate::pricing::CostCurrency::Cny);
            assert_eq!(saved_value, alias);
            assert_eq!(display_value, alias);
        }
    }

    #[test]
    fn config_view_cost_currency_matching_cny_setting_is_silent() {
        let (saved_value, display_value, effective_currency, locale) =
            cost_currency_row_for_settings("locale = \"zh-Hans\"\ncost_currency = \"cny\"\n");

        assert_eq!(locale, Locale::ZhHans);
        assert_eq!(effective_currency, crate::pricing::CostCurrency::Cny);
        assert_eq!(saved_value, "cny");
        assert_eq!(display_value, "cny");
    }

    #[test]
    fn config_view_cost_currency_non_zh_hans_locale_uses_saved_currency() {
        let (saved_value, display_value, effective_currency, locale) =
            cost_currency_row_for_settings("locale = \"en\"\ncost_currency = \"cny\"\n");

        assert_eq!(locale, Locale::En);
        assert_eq!(effective_currency, crate::pricing::CostCurrency::Cny);
        assert_eq!(saved_value, "cny");
        assert_eq!(display_value, "cny");
    }

    #[test]
    fn config_view_exposes_all_available_saved_settings() {
        let app = create_test_app();
        let view = ConfigView::new_for_app(&app);
        let keys: std::collections::HashSet<&str> =
            view.rows.iter().map(|row| row.key.as_str()).collect();

        for (key, _) in Settings::available_settings() {
            assert!(keys.contains(key), "missing native config row for {key}");
        }
    }

    #[test]
    fn config_view_displays_saved_codex_reasoning_effort_label() {
        let _guard = ConfigSettingsEnvGuard::new("reasoning_effort = \"max\"\n");
        let mut app = create_test_app();
        app.api_provider = crate::config::ApiProvider::OpenaiCodex;

        let view = ConfigView::new_for_app(&app);
        let row = view
            .rows
            .iter()
            .find(|row| row.key == "reasoning_effort")
            .expect("reasoning_effort row");

        assert_eq!(row.value, "xhigh");
    }

    #[test]
    fn config_view_editing_localized_default_placeholders_starts_blank() {
        let _guard = ConfigSettingsEnvGuard::new("locale = \"zh-Hans\"\n");
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        for (key, message_id) in [
            ("default_model", MessageId::ConfigDefaultValue),
            ("reasoning_effort", MessageId::ConfigDefaultReasoning),
            ("background_color", MessageId::ConfigDefaultValue),
        ] {
            view.selected = view
                .rows
                .iter()
                .position(|row| row.key == key)
                .unwrap_or_else(|| panic!("{key} row missing"));
            view.start_edit();

            let edit = view.editing.as_ref().expect("editing should start");
            assert_eq!(edit.original_value, tr(Locale::ZhHans, message_id));
            assert!(
                edit.buffer.is_empty(),
                "localized default placeholder should not become edit text for {key}"
            );

            view.editing = None;
        }
    }

    #[test]
    fn config_view_filter_matches_group_and_rows() {
        let mut view = create_config_view(Locale::En);

        type_filter(&mut view, "side");

        assert_eq!(view.filter, "side");
        assert_eq!(visible_section_labels(&view), vec!["Sidebar"]);
        assert_eq!(
            visible_row_keys(&view),
            vec!["sidebar_width", "sidebar_focus", "context_panel"]
        );
        assert_eq!(view.rows[view.selected].key, "sidebar_width");
    }

    #[test]
    fn localized_config_view_filter_matches_english_section_and_scope_labels() {
        let mut view = create_config_view(Locale::PtBr);

        type_filter(&mut view, "sidebar saved");

        assert_eq!(view.filter, "sidebar saved");
        assert_eq!(visible_section_labels(&view), vec!["Barra lateral"]);
        assert_eq!(
            visible_row_keys(&view),
            vec!["sidebar_width", "sidebar_focus", "context_panel"]
        );
    }

    #[test]
    fn config_view_filter_accepts_j_k_and_unicode_case() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        type_filter(&mut view, "thinking");
        assert_eq!(visible_row_keys(&view), vec!["show_thinking"]);

        view.clear_filter();
        view.rows[0].value = "CAFÉ".to_string();
        type_filter(&mut view, "café");
        assert_eq!(visible_row_keys(&view), vec!["provider"]);
    }

    #[test]
    fn config_view_filter_matches_friendly_labels_and_hints() {
        let mut view = create_config_view(Locale::En);

        type_filter(&mut view, "shell access");
        assert_eq!(visible_row_keys(&view), vec!["allow_shell"]);

        view.clear_filter();
        type_filter(&mut view, "reasoning level");
        assert_eq!(visible_row_keys(&view), vec!["reasoning_effort"]);

        view.clear_filter();
        type_filter(&mut view, "fan-out/fan-in");
        assert_eq!(visible_row_keys(&view), vec!["workflow"]);
    }

    #[test]
    fn config_view_renders_friendly_setting_labels() {
        let view = create_config_view(Locale::En);
        let area = Rect::new(0, 0, 100, 40);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains("Active provider"),
            "missing provider label:\n{dump}"
        );
        assert!(
            dump.contains("Shell access"),
            "missing shell label:\n{dump}"
        );
        assert!(dump.contains("Setting"), "missing table heading:\n{dump}");
    }

    #[test]
    fn localized_config_view_renders_at_narrow_width() {
        let mut app = create_test_app();
        app.ui_locale = Locale::PtBr;
        let view = ConfigView::new_for_app(&app);
        let area = Rect::new(0, 0, 60, 18);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains("Configuração") || dump.contains("Configura"),
            "missing localized config title:\n{dump}"
        );
        assert!(
            !dump.contains("MISSING"),
            "missing-key marker leaked:\n{dump}"
        );
    }

    #[test]
    fn config_view_selected_row_uses_muted_selection_highlight() {
        let mut view = create_config_view(Locale::En);
        view.selected = view
            .rows
            .iter()
            .position(|row| row.key == "theme")
            .expect("theme row");
        view.adjust_scroll(8);
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let y = view
            .last_row_hitboxes
            .borrow()
            .iter()
            .find_map(|(y, idx)| (*idx == view.selected).then_some(*y))
            .expect("selected config row should have a hitbox");
        let highlighted_cells = (area.x..area.x.saturating_add(area.width))
            .filter(|&x| {
                let cell = &buf[(x, y)];
                !cell.symbol().trim().is_empty()
                    && cell.bg == palette::SELECTION_BG
                    && cell.fg == palette::SELECTION_TEXT
            })
            .count();

        assert!(
            highlighted_cells >= 4,
            "selected config row should render readable selection text"
        );
        assert!(
            !(area.x..area.x.saturating_add(area.width))
                .any(|x| buf[(x, y)].bg == palette::WHALE_ACCENT_PRIMARY),
            "selected config row should not use the bright accent background"
        );
    }

    #[test]
    fn config_view_keeps_scope_column_aligned_for_long_keys() {
        let mut view = create_config_view(Locale::ZhHans);
        type_filter(&mut view, "composer");
        let area = Rect::new(0, 0, 100, 24);
        let mut buf = Buffer::empty(area);

        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains("Paste detection"),
            "friendly config labels should stay readable:\n{dump}"
        );
        let scope_columns = dump
            .lines()
            .filter(|line| {
                line.contains("Composer")
                    || line.contains("Bracketed paste")
                    || line.contains("Paste detection")
            })
            .filter_map(|line| line.find('已'))
            .collect::<Vec<_>>();
        assert!(
            scope_columns.len() >= 3,
            "expected composer config rows with scopes:\n{dump}"
        );
        assert!(
            scope_columns
                .iter()
                .all(|column| *column == scope_columns[0]),
            "scope column should stay aligned even for long keys:\n{dump}"
        );
    }

    #[test]
    fn config_view_filter_no_match_does_not_edit_hidden_row() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        type_filter(&mut view, "zzzz");
        assert!(visible_row_keys(&view).is_empty());

        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
        assert!(view.editing.is_none());

        let clear = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(clear, ViewAction::None));
        assert!(view.filter.is_empty());
        assert!(!visible_row_keys(&view).is_empty());
    }

    #[test]
    fn config_view_can_edit_filtered_row() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        type_filter(&mut view, "mcp_config");
        assert_eq!(visible_row_keys(&view), vec!["mcp_config_path"]);

        let start = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(start, ViewAction::None));
        assert!(view.editing.is_some());

        let clear = view.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(matches!(clear, ViewAction::None));
        type_filter(&mut view, "servers.json");

        let submit = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match submit {
            ViewAction::Emit(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "mcp_config_path");
                assert_eq!(value, "servers.json");
                assert!(persist);
            }
            other => panic!("expected config update emit, got {other:?}"),
        }
    }

    #[test]
    fn config_view_enter_and_ctrl_u_emit_config_updated() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        // Navigate to the "model" row (index 2, after provider and base_url)
        for _ in 0..2 {
            view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        assert_eq!(view.rows[view.selected].key, "model");

        let start = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(start, ViewAction::None));
        assert!(view.editing.is_some());

        let clear = view.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(matches!(clear, ViewAction::None));
        let cleared = view
            .editing
            .as_ref()
            .expect("editing should remain active after Ctrl+U");
        assert!(cleared.buffer.is_empty());

        for ch in "deepseek-v4-flash".chars() {
            let action = view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            assert!(matches!(action, ViewAction::None));
        }

        let submit = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match submit {
            ViewAction::Emit(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "model");
                assert_eq!(value, "deepseek-v4-flash");
                assert!(!persist);
            }
            other => panic!("expected config update emit, got {other:?}"),
        }
        assert!(view.editing.is_none());
    }

    #[test]
    fn config_view_mouse_click_selects_row() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);
        let area = Rect::new(0, 0, 100, 30);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let hitboxes = view.last_row_hitboxes.borrow().clone();
        let (_, row_idx) = hitboxes
            .iter()
            .find(|(_, idx)| {
                view.rows
                    .get(*idx)
                    .is_some_and(|row| row.key == "default_model")
            })
            .copied()
            .expect("default_model row should have a hitbox");
        let y = hitboxes
            .iter()
            .find_map(|(y, idx)| (*idx == row_idx).then_some(*y))
            .expect("selected row should have a y coordinate");

        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 20,
            row: y,
            modifiers: KeyModifiers::NONE,
        });

        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.selected, row_idx);
    }

    #[test]
    fn config_view_bottom_hint_semantically_truncates_at_narrow_width() {
        // The dense bottom status line must truncate on a word boundary with an
        // ellipsis instead of leaving a mid-word fragment clipped by the
        // terminal (#3987).
        let mut app = create_test_app();
        app.ui_locale = Locale::En;
        let mut view = ConfigView::new_for_app(&app);
        view.status = Some(
            "CFGSTATUS persisted the configuration override to disk successfully \
             without clipping the trailing MARKEREND status text"
                .to_string(),
        );

        let area = Rect::new(0, 0, 100, 40);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let rows: Vec<String> = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();

        // No rendered row may overflow the available columns.
        for (idx, row) in rows.iter().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(row) <= usize::from(area.width),
                "line {idx} overflows: {row:?}"
            );
        }

        let status_line = rows
            .iter()
            .find(|row| row.contains("CFGSTATUS"))
            .expect("bottom status hint should be rendered");
        assert!(
            status_line.contains('…'),
            "status should be truncated with an ellipsis: {status_line:?}"
        );
        assert!(
            !status_line.contains("MARKEREND"),
            "truncated status must drop trailing text: {status_line:?}"
        );
    }

    #[test]
    fn config_view_typing_replaces_on_first_char() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        let _ = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let edit = view.editing.as_ref().expect("editing should be active");
        assert!(edit.select_all, "editor should start with select-all");

        let _ = view.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        let edit = view.editing.as_ref().expect("editing should remain active");
        assert_eq!(edit.buffer.iter().collect::<String>(), "x");
    }

    #[test]
    fn config_view_escape_cancels_editing() {
        let mut app = create_test_app();
        app.ui_locale = Locale::En;
        let mut view = ConfigView::new_for_app(&app);
        let _ = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(view.editing.is_some());

        let cancel = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(cancel, ViewAction::None));
        assert!(view.editing.is_none());
        assert_eq!(
            view.status.as_deref(),
            Some(&*tr(Locale::En, MessageId::ConfigEditCancelled))
        );
    }

    /// A modal that doesn't override `handle_paste` must report
    /// "not consumed" so the host can fall through to the composer.
    /// Regression: views/mod.rs previously inverted the boolean, swallowing
    /// every Cmd-V while any modal was on top.
    #[test]
    fn default_modal_does_not_consume_paste() {
        let mut stack = ViewStack::new();
        stack.push(HelpView::new_for_locale(crate::localization::Locale::En));
        assert!(!stack.handle_paste("hello"));
        assert_eq!(stack.top_kind(), Some(ModalKind::Help));
    }

    struct BareModal;

    impl ModalView for BareModal {
        fn kind(&self) -> ModalKind {
            ModalKind::ContextMenu
        }

        fn handle_key(&mut self, _key: KeyEvent) -> ViewAction {
            ViewAction::None
        }

        fn render(&self, area: Rect, buf: &mut Buffer) {
            let x = area.x + area.width / 2;
            let y = area.y + area.height / 2;
            buf[(x, y)]
                .set_symbol("M")
                .set_style(Style::default().fg(Color::White).bg(Color::Red));
        }

        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    #[test]
    fn view_stack_paints_opaque_backdrop_before_modal() {
        let area = Rect::new(0, 0, 24, 8);
        let modal_x = area.x + area.width / 2;
        let modal_y = area.y + area.height / 2;
        let mut buf = Buffer::empty(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)]
                    .set_symbol("X")
                    .set_style(Style::default().fg(Color::Red).bg(Color::Blue));
            }
        }

        let mut stack = ViewStack::new();
        stack.push(BareModal);
        stack.render(area, &mut buf);

        assert_eq!(buf[(modal_x, modal_y)].symbol(), "M");
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if x == modal_x && y == modal_y {
                    continue;
                }
                let cell = &buf[(x, y)];
                assert_eq!(
                    cell.symbol(),
                    " ",
                    "stale glyph at ({x},{y}) must be cleared"
                );
                assert_eq!(
                    cell.bg,
                    palette::WHALE_BG,
                    "backdrop at ({x},{y}) must be opaque"
                );
            }
        }
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
