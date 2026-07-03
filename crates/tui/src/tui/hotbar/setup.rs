use std::collections::{BTreeMap, BTreeSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::config::Config;
use crate::palette;
use crate::tui::app::App;
use crate::tui::views::{
    ActionHint, EmptyState, ListDetailLayout, ModalKind, ModalView, ViewAction, ViewEvent,
    centered_modal_area, render_modal_footer, render_modal_surface,
};

use super::actions::{
    HotbarActionCategory, HotbarActionMetadata, HotbarArgsBehavior, HotbarRecommendationOptions,
    HotbarSafetyClass, hotbar_source_descriptors, recommend_hotbar_actions,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarSetupActionRow {
    pub metadata: HotbarActionMetadata,
    pub disabled_reason: Option<String>,
}

impl HotbarSetupActionRow {
    fn status_label(&self) -> &'static str {
        if self.disabled_reason.is_some() {
            "disabled"
        } else if matches!(self.metadata.args, HotbarArgsBehavior::Required) {
            "prefill"
        } else {
            "ready"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarSetupView {
    sources: Vec<HotbarActionCategory>,
    actions: Vec<HotbarSetupActionRow>,
    selected_source_idx: usize,
    selected_action_idx_by_source: BTreeMap<HotbarActionCategory, usize>,
    selected_slot: u8,
    original_bindings: BTreeMap<u8, codewhale_config::HotbarBindingToml>,
    draft_bindings: BTreeMap<u8, codewhale_config::HotbarBindingToml>,
    recommended_action_ids: BTreeSet<String>,
    validation_errors: Vec<String>,
    query: String,
    filter_focused: bool,
    help_visible: bool,
}

impl HotbarSetupView {
    #[must_use]
    pub fn new(app: &App, config: &Config) -> Self {
        let mut actions = app
            .hotbar_actions
            .iter()
            .map(|action| {
                let metadata = action.metadata(app.ui_locale);
                let disabled_reason = action.disabled_reason(app);
                HotbarSetupActionRow {
                    metadata,
                    disabled_reason,
                }
            })
            .collect::<Vec<_>>();
        actions.sort_by(|a, b| {
            a.metadata
                .category
                .cmp(&b.metadata.category)
                .then_with(|| {
                    a.metadata
                        .display_name
                        .to_ascii_lowercase()
                        .cmp(&b.metadata.display_name.to_ascii_lowercase())
                })
                .then_with(|| a.metadata.id.cmp(&b.metadata.id))
        });

        let mut source_set = hotbar_source_descriptors()
            .iter()
            .map(|descriptor| descriptor.category)
            .collect::<BTreeSet<_>>();
        source_set.extend(actions.iter().map(|row| row.metadata.category));
        let sources = source_set.iter().copied().collect::<Vec<_>>();
        let recommended_action_ids =
            recommend_hotbar_actions(app, HotbarRecommendationOptions::for_setup_wizard())
                .into_iter()
                .map(|entry| entry.metadata.id)
                .collect::<BTreeSet<_>>();

        let known_action_ids = app
            .hotbar_actions
            .iter()
            .map(|action| action.id())
            .collect::<Vec<_>>();
        let original_bindings = config
            .resolve_hotbar_bindings(&known_action_ids)
            .bindings
            .into_iter()
            .map(|binding| {
                (
                    binding.slot,
                    codewhale_config::HotbarBindingToml {
                        slot: binding.slot,
                        action: binding.action,
                        label: binding.label,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        Self {
            sources,
            actions,
            selected_source_idx: 0,
            selected_action_idx_by_source: BTreeMap::new(),
            selected_slot: 1,
            draft_bindings: original_bindings.clone(),
            original_bindings,
            recommended_action_ids,
            validation_errors: Vec::new(),
            query: String::new(),
            filter_focused: false,
            help_visible: false,
        }
    }

    #[must_use]
    #[cfg(test)]
    pub fn source_categories(&self) -> &[HotbarActionCategory] {
        &self.sources
    }

    #[must_use]
    pub fn selected_source(&self) -> Option<HotbarActionCategory> {
        self.sources.get(self.selected_source_idx).copied()
    }

    #[must_use]
    #[cfg(test)]
    pub fn selected_slot(&self) -> u8 {
        self.selected_slot
    }

    #[must_use]
    pub fn selected_action(&self) -> Option<&HotbarSetupActionRow> {
        let source = self.selected_source()?;
        self.actions_for_source(source)
            .get(self.selected_action_idx(source))
            .copied()
    }

    #[must_use]
    #[cfg(test)]
    pub fn binding_for_slot(&self, slot: u8) -> Option<&codewhale_config::HotbarBindingToml> {
        self.draft_bindings.get(&slot)
    }

    #[must_use]
    #[cfg(test)]
    pub fn checked_action_ids(&self) -> BTreeSet<String> {
        self.draft_bindings
            .values()
            .map(|binding| binding.action.clone())
            .collect()
    }

    #[must_use]
    #[cfg(test)]
    pub fn recommended_action_ids(&self) -> &BTreeSet<String> {
        &self.recommended_action_ids
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.draft_bindings != self.original_bindings
    }

    #[must_use]
    #[cfg(test)]
    pub fn validation_errors(&self) -> &[String] {
        &self.validation_errors
    }

    #[must_use]
    #[cfg(test)]
    pub fn query(&self) -> &str {
        &self.query
    }

    #[must_use]
    pub fn status_text(&self) -> String {
        if let Some(error) = self.validation_errors.last() {
            return error.clone();
        }
        let dirty = if self.is_dirty() { "modified" } else { "clean" };
        let action = self
            .selected_action()
            .map(|row| format!("{} ({})", row.metadata.display_name, row.status_label()))
            .unwrap_or_else(|| "No action".to_string());
        format!("slot {} | {action} | {dirty}", self.selected_slot)
    }

    #[cfg(test)]
    pub fn select_action_by_id(&mut self, action_id: &str) -> bool {
        self.query.clear();
        self.filter_focused = false;
        let Some(row) = self
            .actions
            .iter()
            .find(|row| row.metadata.id == action_id)
            .cloned()
        else {
            return false;
        };
        let Some(source_idx) = self
            .sources
            .iter()
            .position(|source| *source == row.metadata.category)
        else {
            return false;
        };
        self.selected_source_idx = source_idx;
        let index = self
            .actions_for_source(row.metadata.category)
            .iter()
            .position(|candidate| candidate.metadata.id == action_id)
            .unwrap_or(0);
        self.selected_action_idx_by_source
            .insert(row.metadata.category, index);
        self.validation_errors.clear();
        true
    }

    pub fn select_slot(&mut self, slot: u8) -> bool {
        if !(1..=codewhale_config::HOTBAR_SLOT_COUNT).contains(&slot) {
            self.validation_errors = vec![format!(
                "Hotbar slot {slot} is outside 1-{}",
                codewhale_config::HOTBAR_SLOT_COUNT
            )];
            return false;
        }
        self.selected_slot = slot;
        self.validation_errors.clear();
        true
    }

    pub fn assign_selected_action(&mut self) -> bool {
        let Some(row) = self.selected_action().cloned() else {
            self.validation_errors = vec!["No action selected.".to_string()];
            return false;
        };
        if let Some(reason) = row.disabled_reason {
            self.validation_errors = vec![format!(
                "{} cannot be assigned: {reason}",
                row.metadata.display_name
            )];
            return false;
        }
        self.draft_bindings.insert(
            self.selected_slot,
            codewhale_config::HotbarBindingToml {
                slot: self.selected_slot,
                action: row.metadata.id,
                label: None,
            },
        );
        self.validation_errors.clear();
        true
    }

    pub fn toggle_selected_action(&mut self) -> bool {
        let selected_id = self
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .unwrap_or_default();
        if self
            .draft_bindings
            .get(&self.selected_slot)
            .is_some_and(|binding| binding.action == selected_id)
        {
            self.clear_selected_slot();
            true
        } else {
            self.assign_selected_action()
        }
    }

    pub fn clear_selected_slot(&mut self) {
        self.draft_bindings.remove(&self.selected_slot);
        self.validation_errors.clear();
    }

    #[must_use]
    pub fn save_bindings(&self) -> Vec<codewhale_config::HotbarBindingToml> {
        self.draft_bindings.values().cloned().collect()
    }

    fn actions_for_source(&self, source: HotbarActionCategory) -> Vec<&HotbarSetupActionRow> {
        let query = self.query.trim().to_ascii_lowercase();
        self.actions
            .iter()
            .filter(|row| {
                row.metadata.category == source
                    && (query.is_empty() || action_matches_query(row, &query))
            })
            .collect()
    }

    fn unfiltered_actions_for_source(
        &self,
        source: HotbarActionCategory,
    ) -> Vec<&HotbarSetupActionRow> {
        self.actions
            .iter()
            .filter(|row| row.metadata.category == source)
            .collect()
    }

    fn selected_action_idx(&self, source: HotbarActionCategory) -> usize {
        let len = self.actions_for_source(source).len();
        if len == 0 {
            return 0;
        }
        self.selected_action_idx_by_source
            .get(&source)
            .copied()
            .unwrap_or(0)
            .min(len.saturating_sub(1))
    }

    fn set_selected_action_idx(&mut self, source: HotbarActionCategory, idx: usize) {
        let len = self.actions_for_source(source).len();
        if len == 0 {
            self.selected_action_idx_by_source.insert(source, 0);
        } else {
            self.selected_action_idx_by_source
                .insert(source, idx.min(len.saturating_sub(1)));
        }
    }

    fn move_source(&mut self, delta: isize) {
        if self.sources.is_empty() {
            return;
        }
        self.selected_source_idx = wrap_index(self.selected_source_idx, self.sources.len(), delta);
        self.validation_errors.clear();
    }

    fn move_action(&mut self, delta: isize) {
        let Some(source) = self.selected_source() else {
            return;
        };
        let len = self.actions_for_source(source).len();
        if len == 0 {
            return;
        }
        let next = wrap_index(self.selected_action_idx(source), len, delta);
        self.set_selected_action_idx(source, next);
        self.validation_errors.clear();
    }

    fn move_slot(&mut self, delta: isize) {
        let len = usize::from(codewhale_config::HOTBAR_SLOT_COUNT);
        let next = wrap_index(usize::from(self.selected_slot - 1), len, delta) + 1;
        self.selected_slot = u8::try_from(next).expect("hotbar slot fits in u8");
        self.validation_errors.clear();
    }

    fn save_action(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::HotbarSetupSaved {
            bindings: self.save_bindings(),
        })
    }

    #[cfg(test)]
    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.extend(self.header_lines());

        let Some(source) = self.selected_source() else {
            lines.push(Line::from("No hotbar actions are available."));
            return lines;
        };

        for (idx, row) in self.actions_for_source(source).iter().enumerate() {
            lines.push(self.action_row_line(source, idx, row));
        }

        lines.push(Line::from(""));
        lines.push(self.slots_line());
        lines.push(Line::from(self.status_text()));
        lines
    }

    fn header_lines(&self) -> Vec<Line<'static>> {
        let alt_prefix = crate::tui::widgets::key_hint::alt_prefix();
        vec![
            Line::from(Span::styled(
                format!(
                    "Hotbar gives you {alt_prefix}1-8 shortcuts. Assign actions below; \
                     press 'd' or run `/hotbar off` to hide it."
                ),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::DIM),
            )),
            self.slots_line(),
            self.source_tabs_line(),
            self.filter_line(),
            Line::from(self.status_text()),
        ]
    }

    fn source_tabs_line(&self) -> Line<'static> {
        let mut spans = Vec::new();
        for (idx, source) in self.sources.iter().enumerate() {
            if idx > 0 {
                spans.push(Span::raw("  "));
            }
            let count = self.unfiltered_actions_for_source(*source).len();
            let label = if Some(*source) == self.selected_source() {
                format!("[{} {count}]", source.as_str())
            } else {
                format!("{} {count}", source.as_str())
            };
            spans.push(Span::styled(
                label,
                Style::default()
                    .fg(if Some(*source) == self.selected_source() {
                        Color::Cyan
                    } else {
                        palette::TEXT_MUTED
                    })
                    .add_modifier(if Some(*source) == self.selected_source() {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ));
        }
        Line::from(spans)
    }

    fn filter_line(&self) -> Line<'static> {
        let value = if self.query.is_empty() {
            if self.filter_focused {
                "type to filter".to_string()
            } else {
                "press / or type to filter".to_string()
            }
        } else {
            self.query.clone()
        };
        Line::from(vec![
            Span::styled("Filter ", Style::default().fg(palette::TEXT_MUTED)),
            Span::styled(
                value,
                Style::default().fg(if self.filter_focused {
                    palette::DEEPSEEK_SKY
                } else {
                    palette::TEXT_PRIMARY
                }),
            ),
        ])
    }

    fn slots_line(&self) -> Line<'static> {
        let slots = (1..=codewhale_config::HOTBAR_SLOT_COUNT)
            .map(|slot| {
                let label = self
                    .draft_bindings
                    .get(&slot)
                    .map(|binding| compact_action_id(&binding.action))
                    .unwrap_or_else(|| "empty".to_string());
                if slot == self.selected_slot {
                    format!("[{slot}:{label}]")
                } else {
                    format!("{slot}:{label}")
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        Line::from(slots)
    }

    fn action_row_line(
        &self,
        source: HotbarActionCategory,
        idx: usize,
        row: &HotbarSetupActionRow,
    ) -> Line<'static> {
        let selected = idx == self.selected_action_idx(source);
        let marker = if selected { ">" } else { " " };
        let checked = if self
            .draft_bindings
            .values()
            .any(|binding| binding.action == row.metadata.id)
        {
            "*"
        } else {
            " "
        };
        let recommended = if self.recommended_action_ids.contains(&row.metadata.id) {
            "rec"
        } else {
            ""
        };
        let mut text = format!(
            "{marker}{checked} {:<3} {:<22} {:<8} {}",
            recommended,
            row.metadata.display_name,
            row.status_label(),
            row.metadata.description
        );
        if let Some(reason) = row.disabled_reason.as_deref() {
            text.push_str(" (");
            text.push_str(reason);
            text.push(')');
        }
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(if selected {
                    palette::DEEPSEEK_SKY
                } else {
                    palette::TEXT_PRIMARY
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ))
    }

    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.header_lines())
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    fn render_action_list(&self, area: Rect, buf: &mut Buffer) {
        let Some(source) = self.selected_source() else {
            EmptyState::new("No actions", "No hotbar action sources are available.")
                .render(area, buf);
            return;
        };
        let rows = self.actions_for_source(source);
        if rows.is_empty() {
            let (title, body) = if self.query.trim().is_empty()
                && hotbar_source_descriptors()
                    .iter()
                    .find(|descriptor| descriptor.category == source)
                    .is_some_and(|descriptor| !descriptor.registers_dispatchable_actions())
            {
                (
                    "Source not bindable yet",
                    "This source is exploratory until approval-gated activation is wired.",
                )
            } else {
                (
                    "No matching actions",
                    "Clear the filter or switch categories to find another bindable action.",
                )
            };
            EmptyState::new(title, body)
                .primary_action("/", "filter")
                .secondary_action("Esc", "clear filter")
                .render(area, buf);
            return;
        }
        let mut lines = vec![Line::from(Span::styled(
            format!("{} actions", source.as_str()),
            Style::default()
                .fg(palette::TEXT_MUTED)
                .add_modifier(Modifier::BOLD),
        ))];
        for (idx, row) in rows.iter().enumerate() {
            lines.push(self.action_row_line(source, idx, row));
        }
        Paragraph::new(lines)
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .render(area, buf);
    }

    fn render_action_detail(&self, area: Rect, buf: &mut Buffer) {
        let Some(row) = self.selected_action() else {
            EmptyState::new(
                "Select an action",
                "Move through the catalog to preview the selected slot binding.",
            )
            .primary_action("Tab", "category")
            .secondary_action("/", "filter")
            .render(area, buf);
            return;
        };
        Paragraph::new(self.detail_lines(row))
            .style(Style::default().fg(palette::TEXT_PRIMARY))
            .wrap(Wrap { trim: true })
            .render(area, buf);
    }

    fn detail_lines(&self, row: &HotbarSetupActionRow) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(
                row.metadata.display_name.clone(),
                Style::default()
                    .fg(palette::TEXT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                row.metadata.id.clone(),
                Style::default().fg(palette::TEXT_MUTED),
            )),
            Line::from(""),
            Line::from(format!("Category: {}", row.metadata.category.as_str())),
            Line::from(format!("Status: {}", row.status_label())),
            Line::from(format!("Safety: {}", safety_label(row.metadata.safety))),
            Line::from(format!("Arguments: {}", args_label(row.metadata.args))),
            Line::from(format!(
                "Slot {}: {}",
                self.selected_slot,
                self.selected_slot_binding_label()
            )),
            Line::from(""),
            Line::from(row.metadata.description.clone()),
            Line::from(""),
            Line::from(preview_line(row)),
        ];
        if let Some(reason) = row.disabled_reason.as_deref() {
            lines.push(Line::from(Span::styled(
                format!("Unavailable: {reason}"),
                Style::default().fg(palette::STATUS_WARNING),
            )));
        }
        if self.help_visible {
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Save writes staged slots; Esc cancels staged changes unless a filter is active.",
            ));
            lines.push(Line::from(
                "After save: Alt+1 through Alt+8 dispatch Hotbar slots. Bare 1-8 stay composer text outside setup.",
            ));
        }
        lines
    }

    fn selected_slot_binding_label(&self) -> String {
        let Some(binding) = self.draft_bindings.get(&self.selected_slot) else {
            return "empty".to_string();
        };
        self.actions
            .iter()
            .find(|row| row.metadata.id == binding.action)
            .map(|row| row.metadata.display_name.clone())
            .unwrap_or_else(|| binding.action.clone())
    }
}

impl ModalView for HotbarSetupView {
    fn kind(&self) -> ModalKind {
        ModalKind::HotbarSetup
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc if self.filter_focused || !self.query.is_empty() => {
                self.query.clear();
                self.filter_focused = false;
                self.validation_errors.clear();
                ViewAction::None
            }
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Char('q') | KeyCode::Char('Q')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                ViewAction::Close
            }
            KeyCode::Tab => {
                self.move_source(1);
                ViewAction::None
            }
            KeyCode::BackTab => {
                self.move_source(-1);
                ViewAction::None
            }
            KeyCode::Left if key.modifiers.contains(KeyModifiers::ALT) => {
                self.move_source(-1);
                ViewAction::None
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::ALT) => {
                self.move_source(1);
                ViewAction::None
            }
            KeyCode::Left => {
                self.move_slot(-1);
                ViewAction::None
            }
            KeyCode::Right => {
                self.move_slot(1);
                ViewAction::None
            }
            KeyCode::Up => {
                self.move_action(-1);
                ViewAction::None
            }
            KeyCode::Char('k') | KeyCode::Char('K')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                self.move_action(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_action(1);
                ViewAction::None
            }
            KeyCode::Char('j') | KeyCode::Char('J')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                self.move_action(1);
                ViewAction::None
            }
            KeyCode::Enter => {
                self.assign_selected_action();
                ViewAction::None
            }
            KeyCode::Char('a') | KeyCode::Char('A')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                self.assign_selected_action();
                ViewAction::None
            }
            KeyCode::Char(' ') => {
                self.toggle_selected_action();
                ViewAction::None
            }
            KeyCode::Backspace if self.filter_focused || !self.query.is_empty() => {
                self.query.pop();
                if self.query.is_empty() {
                    self.filter_focused = false;
                }
                self.validation_errors.clear();
                ViewAction::None
            }
            KeyCode::Backspace | KeyCode::Delete => {
                self.clear_selected_slot();
                ViewAction::None
            }
            KeyCode::Char('c') | KeyCode::Char('C')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                self.clear_selected_slot();
                ViewAction::None
            }
            KeyCode::Char(ch) if ('1'..='8').contains(&ch) => {
                let slot = ch.to_digit(10).expect("digit") as u8;
                self.select_slot(slot);
                ViewAction::None
            }
            KeyCode::Char('s') | KeyCode::Char('S')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                self.save_action()
            }
            KeyCode::Char('d') | KeyCode::Char('D')
                if key.modifiers.is_empty() && !self.filter_focused =>
            {
                // "Disable Hotbar" from inside the setup flow: hide it and
                // persist `hotbar = []`. Mirrors `/hotbar off`.
                ViewAction::EmitAndClose(ViewEvent::HotbarDisableRequested)
            }
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                self.filter_focused = true;
                self.validation_errors.clear();
                ViewAction::None
            }
            KeyCode::Char('?') => {
                self.help_visible = !self.help_visible;
                ViewAction::None
            }
            KeyCode::Char(ch) if key.modifiers.is_empty() => {
                self.filter_focused = true;
                self.query.push(ch);
                self.validation_errors.clear();
                if let Some(source) = self.selected_source() {
                    self.set_selected_action_idx(source, 0);
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_area = centered_modal_area(area, 118, 28, 72, 12);
        render_modal_surface(area, popup_area, buf);
        let block = Block::default()
            .title(" Hotbar setup ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK));
        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("Tab/Shift+Tab", "source"),
                ActionHint::new("Up/Down", "action"),
                ActionHint::new("1-8", "slot"),
                ActionHint::new("/", "filter"),
                ActionHint::new("Enter/A", "assign"),
                ActionHint::new("Space", "toggle"),
                ActionHint::new("C/Delete", "clear"),
                ActionHint::new("s", "save"),
                ActionHint::new("d", "disable"),
                ActionHint::new("Esc", "cancel"),
            ],
        );
        let header_height = content.height.min(5);
        let header = Rect {
            x: content.x,
            y: content.y,
            width: content.width,
            height: header_height,
        };
        self.render_header(header, buf);
        let body = Rect {
            x: content.x,
            y: content.y + header_height,
            width: content.width,
            height: content.height.saturating_sub(header_height),
        };
        let layout = ListDetailLayout::split(body, 34);
        self.render_action_list(layout.list, buf);
        self.render_action_detail(layout.detail, buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn wrap_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = isize::try_from(len).expect("len fits in isize");
    let current = isize::try_from(current).expect("current fits in isize");
    usize::try_from((current + delta).rem_euclid(len)).expect("wrapped index fits")
}

fn action_matches_query(row: &HotbarSetupActionRow, query: &str) -> bool {
    [
        row.metadata.id.as_str(),
        row.metadata.display_name.as_str(),
        row.metadata.description.as_str(),
        row.metadata.category.as_str(),
        row.status_label(),
        row.disabled_reason.as_deref().unwrap_or_default(),
    ]
    .into_iter()
    .any(|value| value.to_ascii_lowercase().contains(query))
}

fn safety_label(safety: HotbarSafetyClass) -> &'static str {
    match safety {
        HotbarSafetyClass::LocalUi => "safe UI",
        HotbarSafetyClass::LocalState => "local state",
        HotbarSafetyClass::ConfigChange => "config change",
        HotbarSafetyClass::ExternalInput => "external input",
        HotbarSafetyClass::ExistingCommand => "existing command",
        HotbarSafetyClass::RequiresApproval => "approval gated",
    }
}

fn args_label(args: HotbarArgsBehavior) -> &'static str {
    match args {
        HotbarArgsBehavior::None => "none",
        HotbarArgsBehavior::Optional => "optional",
        HotbarArgsBehavior::Required => "prefill required arguments",
    }
}

fn preview_line(row: &HotbarSetupActionRow) -> String {
    match (row.metadata.category, row.metadata.args) {
        (HotbarActionCategory::Route, _) => {
            "Preview: switches provider/model through /model route logic.".to_string()
        }
        (_, HotbarArgsBehavior::Required) => {
            "Preview: pre-fills the composer instead of running blindly.".to_string()
        }
        _ => "Preview: dispatches through the existing Hotbar action path.".to_string(),
    }
}

fn compact_action_id(action_id: &str) -> String {
    let suffix = action_id.rsplit('.').next().unwrap_or(action_id);
    crate::tui::ui_text::truncate_line_to_width(suffix, 7)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiProvider, Config};
    use crate::tui::app::TuiOptions;
    use crossterm::event::KeyModifiers;
    use std::path::PathBuf;

    fn test_app_with_config(config: &Config) -> App {
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
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, config);
        app.ui_locale = crate::localization::Locale::En;
        app
    }

    fn test_app() -> App {
        test_app_with_config(&Config::default())
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn wizard_sources_follow_registered_action_categories() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        // Deferred sources are visible as adapter boundaries, but they have no
        // bindable rows until their approval/receipt paths are wired.
        assert_eq!(
            view.source_categories(),
            &[
                HotbarActionCategory::App,
                HotbarActionCategory::Mode,
                HotbarActionCategory::Route,
                HotbarActionCategory::Slash,
                HotbarActionCategory::Mcp,
                HotbarActionCategory::Skill,
                HotbarActionCategory::Plugin,
            ]
        );
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::App));
        assert!(view.recommended_action_ids().contains("mode.agent"));
        assert!(!view.select_action_by_id("skill.example"));
        // #3807: a fresh config seeds no bindings, so the wizard opens with
        // nothing checked until the user opts in.
        assert!(view.checked_action_ids().is_empty());
    }

    #[test]
    fn wizard_assigns_replaces_toggles_and_clears_slots() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_slot(1));
        assert!(view.select_action_by_id("mode.plan"));
        assert!(view.assign_selected_action());
        assert_eq!(
            view.binding_for_slot(1)
                .map(|binding| binding.action.as_str()),
            Some("mode.plan")
        );

        assert!(view.select_action_by_id("mode.agent"));
        assert!(view.assign_selected_action());
        assert_eq!(
            view.binding_for_slot(1)
                .map(|binding| binding.action.as_str()),
            Some("mode.agent")
        );
        assert!(view.is_dirty());

        assert!(view.toggle_selected_action());
        assert!(view.binding_for_slot(1).is_none());
        view.clear_selected_slot();
        assert!(view.binding_for_slot(1).is_none());
    }

    #[test]
    fn wizard_save_emits_bindings_but_escape_only_closes() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(view.select_slot(8));
        assert!(view.select_action_by_id("sidebar.toggle"));
        assert!(view.assign_selected_action());

        match view.handle_key(key(KeyCode::Char('s'))) {
            ViewAction::EmitAndClose(ViewEvent::HotbarSetupSaved { bindings }) => {
                assert!(
                    bindings
                        .iter()
                        .any(|binding| { binding.slot == 8 && binding.action == "sidebar.toggle" })
                );
            }
            other => panic!("expected HotbarSetupSaved, got {other:?}"),
        }

        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(view.select_slot(1));
        assert!(view.select_action_by_id("mode.agent"));
        assert!(view.assign_selected_action());
        assert!(matches!(
            view.handle_key(key(KeyCode::Esc)),
            ViewAction::Close
        ));
    }

    #[test]
    fn wizard_disable_key_emits_disable_request_and_intro_mentions_it() {
        let app = test_app();

        // 'd' and 'D' hide the Hotbar from inside the setup flow (mirrors /hotbar off).
        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('d'))),
            ViewAction::EmitAndClose(ViewEvent::HotbarDisableRequested)
        ));
        let mut view = HotbarSetupView::new(&app, &Config::default());
        assert!(matches!(
            view.handle_key(key(KeyCode::Char('D'))),
            ViewAction::EmitAndClose(ViewEvent::HotbarDisableRequested)
        ));

        // The always-visible intro explains what Hotbar is and the disable path.
        let joined: String = view
            .render_lines()
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(
            joined.contains("shortcuts"),
            "intro should explain what Hotbar is: {joined:?}"
        );
        assert!(
            joined.contains("/hotbar off"),
            "intro should mention the disable path: {joined:?}"
        );
    }

    #[test]
    fn disabled_actions_are_visible_but_not_assignable() {
        let mut app = test_app();
        app.auto_model = true;
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_slot(2));
        assert!(view.select_action_by_id("reasoning.cycle"));
        assert!(!view.assign_selected_action());

        assert_ne!(
            view.binding_for_slot(2)
                .map(|binding| binding.action.as_str()),
            Some("reasoning.cycle")
        );
        assert!(
            view.validation_errors()
                .last()
                .is_some_and(|error| error.contains("cannot be assigned"))
        );
        assert!(view.status_text().contains("cannot be assigned"));
    }

    #[test]
    fn args_required_slash_actions_are_visible_and_assignable_as_prefill() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(view.select_action_by_id("slash.rename"));
        assert!(
            view.status_text().contains("prefill"),
            "required-arg commands must be labeled as prefill actions"
        );
        assert!(view.select_slot(3));
        assert!(view.assign_selected_action());

        assert_eq!(
            view.binding_for_slot(3)
                .map(|binding| binding.action.as_str()),
            Some("slash.rename")
        );
    }

    #[test]
    fn wizard_help_documents_runtime_hotbar_shortcut() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        assert!(matches!(
            view.handle_key(key(KeyCode::Char('?'))),
            ViewAction::None
        ));
        let rendered = view
            .selected_action()
            .map(|row| view.detail_lines(row))
            .expect("selected action")
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("After save: Alt+1 through Alt+8 dispatch Hotbar slots"));
        assert!(rendered.contains("Bare 1-8 stay composer text outside setup"));
    }

    #[test]
    fn keyboard_controls_navigate_source_action_and_slot() {
        let mut config = Config {
            provider: Some(ApiProvider::Deepseek.as_str().to_string()),
            ..Config::default()
        };
        config
            .provider_config_for_mut(ApiProvider::Openrouter)
            .model = Some("anthropic/claude-sonnet-4".to_string());
        let app = test_app_with_config(&config);
        let mut view = HotbarSetupView::new(&app, &config);

        assert_eq!(view.selected_source(), Some(HotbarActionCategory::App));
        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Mode));
        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Route));
        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Slash));
        view.handle_key(key(KeyCode::BackTab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Route));

        let first = view
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .expect("first action");
        view.handle_key(key(KeyCode::Down));
        let second = view
            .selected_action()
            .map(|row| row.metadata.id.clone())
            .expect("second action");
        assert_ne!(first, second);

        view.handle_key(key(KeyCode::Char('8')));
        assert_eq!(view.selected_slot(), 8);
        view.handle_key(key(KeyCode::Left));
        assert_eq!(view.selected_slot(), 7);
    }

    #[test]
    fn keyboard_filter_searches_catalog_and_escape_clears_it() {
        let app = test_app();
        let mut view = HotbarSetupView::new(&app, &Config::default());

        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Mode));
        view.handle_key(key(KeyCode::Tab));
        assert_eq!(view.selected_source(), Some(HotbarActionCategory::Route));
        let route_label = view
            .selected_action()
            .map(|row| row.metadata.display_name.clone())
            .expect("route action");
        let route_query = route_label
            .chars()
            .take(4)
            .collect::<String>()
            .to_ascii_lowercase();
        view.handle_key(key(KeyCode::Char('/')));
        for ch in route_query.chars() {
            view.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(view.query(), route_query);
        assert!(view.status_text().contains(&route_label));

        view.handle_key(key(KeyCode::Esc));
        assert_eq!(view.query(), "");
        assert!(matches!(
            view.handle_key(key(KeyCode::Esc)),
            ViewAction::Close
        ));
    }

    #[test]
    fn hotbar_setup_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        use unicode_width::UnicodeWidthStr;

        const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];
        let app = test_app();
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(HotbarSetupView::new(&app, &Config::default()));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
                .collect();
            let text = rows.join("\n");

            // Footer keeps every action.
            for label in [
                "source", "action", "slot", "filter", "assign", "toggle", "clear", "save",
                "disable", "cancel",
            ] {
                assert!(text.contains(label), "{w}x{h}: footer missing '{label}'");
            }

            // Composited frame is fully opaque.
            assert!(!text.contains('X'), "{w}x{h}: background bleed-through");
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::DEEPSEEK_INK,
                "{w}x{h}: modal interior must be opaque"
            );

            // No horizontal overflow.
            for (y, row) in rows.iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }
}
