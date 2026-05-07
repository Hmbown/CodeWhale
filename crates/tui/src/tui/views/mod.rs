use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, layout::Rect};
use std::cell::{Cell, RefCell};
use std::fmt;

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::settings::Settings;
use crate::tools::UserInputResponse;
use crate::tools::subagent::{SubAgentAssignment, SubAgentResult, SubAgentStatus, SubAgentType};
use crate::tui::app::App;
use crate::tui::approval::{ElevationOption, ReviewDecision};
use crate::tui::history::{HistoryCell, SubAgentCell, summarize_tool_output};
use crate::tui::widgets::agent_card::AgentLifecycle;

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
    FilePicker,
    StatusPicker,
    ContextMenu,
    ShellControl,
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
        /// Fingerprint key for per‑call approval caching (§5.A).
        approval_key: String,
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
        effort: crate::tui::app::ReasoningEffort,
        previous_model: String,
        previous_effort: crate::tui::app::ReasoningEffort,
    },
    /// Emitted by the `/provider` picker when the user selects a provider
    /// that already has credentials — the handler should perform the same
    /// switch as `AppAction::SwitchProvider`.
    ProviderPickerApplied {
        provider: crate::config::ApiProvider,
    },
    /// Emitted by the `/provider` picker after the user types an API key
    /// inline for a provider that lacked one. The handler should persist
    /// the key via `save_api_key_for` and then perform the provider switch.
    ProviderPickerApiKeySubmitted {
        provider: crate::config::ApiProvider,
        api_key: String,
    },
    /// Emitted by the `/statusline` picker every time the user toggles an
    /// item (live preview) and once more on Enter (final). The handler
    /// updates `app.status_items` immediately and persists on `final_save`
    /// so the footer animates without a write per keystroke.
    StatusItemsUpdated {
        items: Vec<crate::config::StatusItem>,
        final_save: bool,
    },
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
    ShellControlBackground,
    ShellControlCancel,
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
        tracing::debug!(target: "deepseek_tui::view_stack", action = "push", kind = ?kind, depth = self.views.len(), "view pushed");
    }

    /// Push an already-boxed view back onto the stack. Used by call sites
    /// that pop a view, mutate it externally, and need to restore it without
    /// the generic `push` re-boxing dance.
    pub fn push_boxed(&mut self, view: Box<dyn ModalView>) {
        let kind = view.kind();
        self.views.push(view);
        tracing::debug!(target: "deepseek_tui::view_stack", action = "push_boxed", kind = ?kind, depth = self.views.len(), "view pushed");
    }

    pub fn pop(&mut self) -> Option<Box<dyn ModalView>> {
        let popped = self.views.pop();
        if let Some(view) = popped.as_ref() {
            tracing::debug!(target: "deepseek_tui::view_stack", action = "pop", kind = ?view.kind(), depth = self.views.len(), "view popped");
        }
        popped
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        for view in &self.views {
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
                    tracing::debug!(target: "deepseek_tui::view_stack", action = "close", kind = ?view.kind(), depth = self.views.len(), "view closed via action");
                }
            }
            ViewAction::Emit(event) => {
                events.push(event);
            }
            ViewAction::EmitAndClose(event) => {
                events.push(event);
                if let Some(view) = self.views.pop() {
                    tracing::debug!(target: "deepseek_tui::view_stack", action = "emit_and_close", kind = ?view.kind(), depth = self.views.len(), "view closed via action");
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
enum ShellControlChoice {
    Background,
    Cancel,
}

impl ShellControlChoice {
    fn event(self) -> ViewEvent {
        match self {
            ShellControlChoice::Background => ViewEvent::ShellControlBackground,
            ShellControlChoice::Cancel => ViewEvent::ShellControlCancel,
        }
    }
}

pub struct ShellControlView {
    selected: ShellControlChoice,
}

impl ShellControlView {
    pub fn new() -> Self {
        Self {
            selected: ShellControlChoice::Background,
        }
    }

    fn toggle(&mut self) {
        self.selected = match self.selected {
            ShellControlChoice::Background => ShellControlChoice::Cancel,
            ShellControlChoice::Cancel => ShellControlChoice::Background,
        };
    }
}

impl ModalView for ShellControlView {
    fn kind(&self) -> ModalKind {
        ModalKind::ShellControl
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                self.toggle();
                ViewAction::None
            }
            KeyCode::Char('b') | KeyCode::Char('B') => {
                ViewAction::EmitAndClose(ViewEvent::ShellControlBackground)
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                ViewAction::EmitAndClose(ViewEvent::ShellControlCancel)
            }
            KeyCode::Enter => ViewAction::EmitAndClose(self.selected.event()),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        use ratatui::{
            prelude::Stylize,
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
        };

        let popup_width = 62.min(area.width.saturating_sub(4));
        let popup_height = 11.min(area.height.saturating_sub(2));

        let popup_area = Rect {
            x: (area.width - popup_width) / 2,
            y: (area.height - popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let option_line = |choice: ShellControlChoice, key: &'static str, label: &'static str| {
            let selected = self.selected == choice;
            let style = if selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            Line::from(vec![
                Span::styled(if selected { "> " } else { "  " }, style),
                Span::styled(format!("{key:<3}"), style.bold()),
                Span::styled(label, style),
            ])
        };

        let lines = vec![
            Line::from(Span::styled(
                "Foreground shell command is still running.",
                Style::default().fg(palette::TEXT_PRIMARY),
            )),
            Line::from(""),
            option_line(
                ShellControlChoice::Background,
                "B",
                "Background - detach and keep the command running",
            ),
            option_line(
                ShellControlChoice::Cancel,
                "C",
                "Cancel - stop the command and interrupt this turn",
            ),
        ];

        let view = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(Line::from(vec![Span::styled(
                        " Shell command ",
                        Style::default().fg(palette::DEEPSEEK_BLUE).bold(),
                    )]))
                    .title_bottom(Line::from(Span::styled(
                        " Enter select | Esc close ",
                        Style::default().fg(palette::TEXT_MUTED),
                    )))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette::BORDER_COLOR))
                    .style(Style::default().bg(palette::DEEPSEEK_INK))
                    .padding(Padding::uniform(1)),
            )
            .style(Style::default().fg(palette::TEXT_PRIMARY));

        view.render(popup_area, buf);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigScope {
    Session,
    Saved,
}

impl ConfigScope {
    fn label(self) -> &'static str {
        match self {
            ConfigScope::Session => "SESSION",
            ConfigScope::Saved => "SAVED",
        }
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
    Model,
    Permissions,
    Display,
    Composer,
    Sidebar,
    History,
    Mcp,
}

impl ConfigSection {
    fn label(self) -> &'static str {
        match self {
            ConfigSection::Model => "Model",
            ConfigSection::Permissions => "Permissions",
            ConfigSection::Display => "Display",
            ConfigSection::Composer => "Composer",
            ConfigSection::Sidebar => "Sidebar",
            ConfigSection::History => "History",
            ConfigSection::Mcp => "MCP",
        }
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
    last_visible_rows: Cell<usize>,
    last_row_hitboxes: RefCell<Vec<(u16, usize)>>,
}

impl ConfigView {
    pub fn new_for_app(app: &App) -> Self {
        let settings = Settings::load().unwrap_or_else(|_| Settings::default());
        let rows = vec![
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
                    .unwrap_or("(default)")
                    .to_string(),
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
                section: ConfigSection::Display,
                key: "locale".to_string(),
                value: settings.locale.clone(),
                editable: true,
                scope: ConfigScope::Saved,
            },
            ConfigRow {
                section: ConfigSection::Display,
                key: "background_color".to_string(),
                value: settings
                    .background_color
                    .clone()
                    .unwrap_or_else(|| "(default)".to_string()),
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
                key: "transcript_spacing".to_string(),
                value: settings.transcript_spacing.clone(),
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
                key: "paste_burst_detection".to_string(),
                value: settings.paste_burst_detection.to_string(),
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
                section: ConfigSection::History,
                key: "auto_compact".to_string(),
                value: settings.auto_compact.to_string(),
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
                key: "mcp_config_path".to_string(),
                value: app.mcp_config_path.display().to_string(),
                editable: true,
                scope: ConfigScope::Saved,
            },
        ];

        Self {
            rows,
            selected: 0,
            scroll: 0,
            editing: None,
            filter: String::new(),
            status: None,
            locale: app.ui_locale,
            last_visible_rows: Cell::new(0),
            last_row_hitboxes: RefCell::new(Vec::new()),
        }
    }

    fn tr(&self, id: MessageId) -> &'static str {
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

        let section = row.section.label().to_lowercase();
        let key = row.key.to_lowercase();
        let value = row.value.to_lowercase();
        let scope = row.scope.label().to_lowercase();

        filter.split_whitespace().all(|term| {
            section.contains(term)
                || key.contains(term)
                || value.contains(term)
                || scope.contains(term)
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
                self.status = Some("Edit cancelled".to_string());
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
        let initial_value = if key == "default_model" && original_value == "(default)" {
            String::new()
        } else {
            original_value.clone()
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
}

fn config_hint_for_key(key: &str) -> &'static str {
    match key {
        "model" => "deepseek-v4-pro | deepseek-v4-flash | deepseek-*",
        "approval_mode" => "auto | suggest | never",
        "auto_compact"
        | "calm_mode"
        | "low_motion"
        | "show_thinking"
        | "show_tool_details"
        | "composer_border"
        | "paste_burst_detection" => "on/off, true/false, yes/no, 1/0",
        "composer_density" | "transcript_spacing" => "compact | comfortable | spacious",
        "locale" => "auto | en | ja | zh-Hans | pt-BR",
        "background_color" => "#RRGGBB | default",
        "default_mode" => "agent | plan | yolo",
        "sidebar_width" => "10..=50",
        "sidebar_focus" => "auto | plan | todos | tasks | agents",
        "max_history" => "integer (0 allowed)",
        "default_model" => "deepseek-v4-pro | deepseek-v4-flash | deepseek-* | none/default",
        "mcp_config_path" => "path to mcp.json",
        _ => "",
    }
}

fn render_config_editor_value_line(edit: &ConfigEdit) -> ratatui::text::Line<'static> {
    use ratatui::{
        prelude::Stylize,
        style::Style,
        text::{Line, Span},
    };

    let mut spans = Vec::new();
    spans.push(Span::styled(
        "New: ",
        Style::default().fg(palette::TEXT_MUTED),
    ));

    let cursor_style = Style::default()
        .fg(palette::DEEPSEEK_INK)
        .bg(palette::DEEPSEEK_SKY)
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
            prelude::Stylize,
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
        };

        let popup_width = 84.min(area.width.saturating_sub(4));
        let popup_height = 22.min(area.height.saturating_sub(4));

        let popup_area = Rect {
            x: (area.width - popup_width) / 2,
            y: (area.height - popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let base_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = base_block.inner(popup_area);
        let (lines, footer) = if let Some(edit) = self.editing.as_ref() {
            let mut lines: Vec<Line> = Vec::new();
            lines.push(Line::from(vec![Span::styled(
                format!("Edit {}", edit.key),
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            )]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Scope: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(edit.scope.label()),
            ]));
            lines.push(Line::from(vec![
                Span::styled("Current: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(truncate_view_text(&edit.original_value, 60)),
            ]));
            lines.push(Line::from(""));
            lines.push(render_config_editor_value_line(edit));
            lines.push(Line::from(""));
            let hint = config_hint_for_key(&edit.key);
            if !hint.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Hint: ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw(hint),
                ]));
            }
            (
                lines,
                " Enter=apply, Esc=cancel, Ctrl+U=clear, Ctrl+A=all, \u{2190}/\u{2192}=move "
                    .to_string(),
            )
        } else {
            let content_height = usize::from(inner.height);
            let header_lines = 5usize;
            let bottom_lines = 1usize;
            let visible_rows = content_height
                .saturating_sub(header_lines + bottom_lines)
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

            let mut lines: Vec<Line> = vec![
                Line::from(vec![Span::styled(
                    self.tr(MessageId::ConfigTitle),
                    Style::default().fg(palette::DEEPSEEK_BLUE).bold(),
                )]),
                Line::from(vec![
                    Span::styled("  Search: ", Style::default().fg(palette::TEXT_MUTED)),
                    Span::raw(search_value),
                    Span::styled(
                        format!("  ({match_count}/{})", self.rows.len()),
                        Style::default().fg(palette::TEXT_MUTED),
                    ),
                ]),
                Line::from(""),
                Line::from("  Key                 Value                                    Scope"),
                Line::from("  ----------------------------------------------------------------"),
            ];
            let mut row_hitboxes = Vec::new();

            for item in items.iter().skip(start).take(visible_rows) {
                match item {
                    ConfigListItem::Section(section) => {
                        lines.push(Line::from(Span::styled(
                            format!("  {}", section.label()),
                            Style::default().fg(palette::DEEPSEEK_SKY).bold(),
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
                                .fg(ratatui::style::Color::White)
                                .bg(palette::DEEPSEEK_BLUE)
                                .add_modifier(ratatui::style::Modifier::BOLD)
                        } else {
                            Style::default().fg(palette::TEXT_PRIMARY)
                        };
                        let value = truncate_view_text(&row.value, 44);
                        let mut line = Line::from(format!(
                            "  {:<19} {:<44} {}",
                            row.key,
                            value,
                            row.scope.label()
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

            let bottom_text = if let Some(status) = self.status.as_ref() {
                status.clone()
            } else if !self.filter.is_empty() {
                format!(
                    "{}: {match_count}",
                    self.tr(MessageId::ConfigFilteredSettings)
                )
            } else if scrollable && !items.is_empty() {
                format!(
                    "{} {}-{} / {}",
                    self.tr(MessageId::ConfigShowing),
                    self.scroll.saturating_add(1),
                    end,
                    items.len()
                )
            } else {
                String::new()
            };
            lines.push(Line::from(Span::styled(
                bottom_text,
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
                Style::default().fg(palette::DEEPSEEK_BLUE).bold(),
            )]))
            .title_bottom(Line::from(Span::styled(
                footer,
                Style::default().fg(palette::TEXT_MUTED),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);
        Paragraph::new(lines)
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .scroll((0, 0))
            .render(inner, buf);
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
    }
}

fn live_subagent_result(
    agent_id: &str,
    agent_type: SubAgentType,
    status: SubAgentStatus,
    objective: &str,
    role: Option<&str>,
) -> SubAgentResult {
    SubAgentResult {
        agent_id: agent_id.to_string(),
        agent_type,
        assignment: SubAgentAssignment {
            objective: summarize_tool_output(objective),
            role: role.map(str::to_string),
        },
        model: String::new(),
        nickname: None,
        status,
        result: None,
        steps_taken: 0,
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
            prelude::Stylize,
            style::Style,
            text::{Line, Span},
            widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
        };

        let popup_width = 78.min(area.width.saturating_sub(4));
        let popup_height = 20.min(area.height.saturating_sub(4));

        let popup_area = Rect {
            x: (area.width - popup_width) / 2,
            y: (area.height - popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let mut lines: Vec<Line> = Vec::new();
        let content_width = popup_width.saturating_sub(4) as usize;

        if self.agents.is_empty() {
            lines.push(Line::from(Span::styled(
                "No agents running.",
                Style::default().fg(palette::TEXT_MUTED),
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
                }
            }

            let status_summary = [
                ("Running", running.len(), palette::STATUS_WARNING),
                ("Completed", completed.len(), palette::STATUS_SUCCESS),
                ("Interrupted", interrupted.len(), palette::STATUS_WARNING),
                ("Failed", failed.len(), palette::DEEPSEEK_RED),
                ("Cancelled", cancelled.len(), palette::TEXT_MUTED),
            ];

            lines.push(Line::from(Span::styled(
                "Sub-agents",
                Style::default().fg(palette::DEEPSEEK_SKY).bold(),
            )));

            let mut summary_parts = Vec::new();
            for (label, count, color) in status_summary {
                summary_parts.push(Line::from(Span::styled(
                    format!("{}: {}", label, count),
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
                palette::DEEPSEEK_RED.into(),
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

        let total_lines = lines.len();
        let visible_lines = (popup_height as usize).saturating_sub(3);
        let max_scroll = total_lines.saturating_sub(visible_lines);
        let scroll = self.scroll.min(max_scroll);

        let scroll_indicator = if total_lines > visible_lines {
            format!(" [{}/{} ↑↓] ", scroll + 1, max_scroll + 1)
        } else {
            String::new()
        };

        let view = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(Line::from(vec![Span::styled(
                        " Sub-agents ",
                        Style::default().fg(palette::DEEPSEEK_BLUE).bold(),
                    )]))
                    .title_bottom(Line::from(vec![
                        Span::styled(" Esc to close ", Style::default().fg(palette::TEXT_MUTED)),
                        Span::styled(" R to refresh ", Style::default().fg(palette::TEXT_MUTED)),
                        Span::styled(scroll_indicator, Style::default().fg(palette::DEEPSEEK_SKY)),
                    ]))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(palette::BORDER_COLOR))
                    .style(Style::default().bg(palette::DEEPSEEK_INK))
                    .padding(Padding::uniform(1)),
            )
            .scroll((scroll as u16, 0));

        view.render(popup_area, buf);
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
        prelude::Stylize,
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
        let kind = format_agent_type(&agent.agent_type);
        let (status, status_style, status_detail) = format_agent_status(&agent.status);

        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{id:<12}"),
                Style::default().fg(palette::TEXT_PRIMARY),
            ),
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
                Span::styled(detail, Style::default().fg(palette::DEEPSEEK_RED)),
            ]));
        }

        if let Some(role) = agent.assignment.role.as_deref() {
            let max_len = content_width.saturating_sub(14);
            let role = truncate_view_text(role, max_len);
            lines.push(Line::from(vec![
                Span::styled("    role: ", Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(role, Style::default().fg(palette::DEEPSEEK_SKY)),
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
        SubAgentStatus::Running => ("running", Style::default().fg(palette::DEEPSEEK_SKY), None),
        SubAgentStatus::Completed => (
            "completed",
            Style::default().fg(palette::DEEPSEEK_BLUE),
            None,
        ),
        SubAgentStatus::Interrupted(reason) => (
            "interrupted",
            Style::default().fg(palette::STATUS_WARNING),
            Some(reason.as_str()),
        ),
        SubAgentStatus::Cancelled => ("cancelled", Style::default().fg(palette::TEXT_MUTED), None),
        SubAgentStatus::Failed(reason) => (
            "failed",
            Style::default().fg(palette::DEEPSEEK_RED),
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
        ConfigListItem, ConfigSection, ConfigView, ModalKind, ModalView, ShellControlView,
        ViewAction, ViewEvent, ViewStack, subagent_view_agents, truncate_view_text,
    };
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::tools::subagent::{
        SubAgentAssignment, SubAgentResult, SubAgentStatus, SubAgentType,
    };
    use crate::tui::app::{App, TuiOptions};
    use crate::tui::history::{HistoryCell, SubAgentCell};
    use crate::tui::widgets::agent_card::{AgentLifecycle, FanoutCard};
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::{buffer::Buffer, layout::Rect};
    use std::path::PathBuf;

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

    fn type_filter(view: &mut ConfigView, text: &str) {
        for ch in text.chars() {
            let action = view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
            assert!(matches!(action, ViewAction::None));
        }
    }

    fn manager_agent(id: &str, status: SubAgentStatus) -> SubAgentResult {
        SubAgentResult {
            agent_id: id.to_string(),
            agent_type: SubAgentType::Explore,
            assignment: SubAgentAssignment {
                objective: "read the docs".to_string(),
                role: None,
            },
            model: "deepseek-v4-flash".to_string(),
            nickname: None,
            status,
            result: None,
            steps_taken: 1,
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

    fn visible_section_labels(view: &ConfigView) -> Vec<&'static str> {
        view.visible_items()
            .into_iter()
            .filter_map(|item| match item {
                ConfigListItem::Section(section) => Some(section.label()),
                ConfigListItem::Row(_) => None,
            })
            .collect()
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
        let app = create_test_app();
        let view = ConfigView::new_for_app(&app);
        assert_eq!(
            visible_section_labels(&view),
            vec![
                ConfigSection::Model.label(),
                ConfigSection::Permissions.label(),
                ConfigSection::Display.label(),
                ConfigSection::Composer.label(),
                ConfigSection::Sidebar.label(),
                ConfigSection::History.label(),
                ConfigSection::Mcp.label(),
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
        assert!(keys.contains(&"model"));
        assert!(keys.contains(&"approval_mode"));
        assert!(keys.contains(&"locale"));
        assert!(keys.contains(&"background_color"));
        assert!(keys.contains(&"auto_compact"));
        assert!(keys.contains(&"composer_border"));
        assert!(keys.contains(&"mcp_config_path"));
        assert!(view.rows.iter().all(|row| row.editable));
    }

    #[test]
    fn config_view_filter_matches_group_and_rows() {
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);

        type_filter(&mut view, "side");

        assert_eq!(view.filter, "side");
        assert_eq!(visible_section_labels(&view), vec!["Sidebar"]);
        assert_eq!(
            visible_row_keys(&view),
            vec!["sidebar_width", "sidebar_focus"]
        );
        assert_eq!(view.rows[view.selected].key, "sidebar_width");
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
        assert_eq!(visible_row_keys(&view), vec!["model"]);
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

        type_filter(&mut view, "mcp");
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
        let app = create_test_app();
        let mut view = ConfigView::new_for_app(&app);
        let _ = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(view.editing.is_some());

        let cancel = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(cancel, ViewAction::None));
        assert!(view.editing.is_none());
        assert_eq!(view.status.as_deref(), Some("Edit cancelled"));
    }

    #[test]
    fn shell_control_view_defaults_to_background() {
        let mut view = ShellControlView::new();

        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ShellControlBackground)
        ));
    }

    #[test]
    fn shell_control_view_can_select_cancel() {
        let mut view = ShellControlView::new();

        let action = view.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));

        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ShellControlCancel)
        ));
    }

    /// A modal that doesn't override `handle_paste` must report
    /// "not consumed" so the host can fall through to the composer.
    /// Regression: views/mod.rs previously inverted the boolean, swallowing
    /// every Cmd-V while any modal was on top.
    #[test]
    fn default_modal_does_not_consume_paste() {
        let mut stack = ViewStack::new();
        stack.push(ShellControlView::new());
        assert!(!stack.handle_paste("hello"));
        assert_eq!(stack.top_kind(), Some(ModalKind::ShellControl));
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
