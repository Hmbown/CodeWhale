//! Tab switcher view for quick tab navigation

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    widgets::{Block, Borders, Widget},
};

use crate::tui::app::App;
use crate::tui::tab::{TabId, TabMetadata};
use crate::tui::views::{ModalKind, ModalView, ViewAction};

/// Tab switcher widget for quick tab navigation
pub struct TabSwitcherView {
    /// All available tabs
    tabs: Vec<TabMetadata>,
    /// Currently selected tab index
    cursor: usize,
    /// Filter string for searching tabs
    filter: String,
}

impl TabSwitcherView {
    /// Create a new tab switcher with the current app tabs
    pub fn new(app: &App) -> Self {
        let tabs: Vec<TabMetadata> = app.tab_manager.all_tabs().into_iter().cloned().collect();
        let cursor = app.tab_manager.active_index().unwrap_or(0);
        let max_cursor = tabs.len().saturating_sub(1);

        Self {
            tabs,
            cursor: cursor.min(max_cursor),
            filter: String::new(),
        }
    }

    /// Get the currently selected tab ID
    #[allow(dead_code)]
    pub fn selected_tab_id(&self) -> Option<TabId> {
        self.tabs.get(self.cursor).map(|t| t.id)
    }

    /// Internal key handler
    pub fn handle_key_internal(&mut self, key: KeyEvent) -> TabSwitcherAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                TabSwitcherAction::Update
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor < self.tabs.len().saturating_sub(1) {
                    self.cursor += 1;
                }
                TabSwitcherAction::Update
            }
            KeyCode::Home => {
                self.cursor = 0;
                TabSwitcherAction::Update
            }
            KeyCode::End => {
                self.cursor = self.tabs.len().saturating_sub(1);
                TabSwitcherAction::Update
            }
            KeyCode::Enter => TabSwitcherAction::Select(self.cursor),
            KeyCode::Esc => TabSwitcherAction::Cancel,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                TabSwitcherAction::Cancel
            }
            // Number keys 1-9 for direct selection
            KeyCode::Char(c) if ('1'..='9').contains(&c) => {
                let index = c.to_digit(10).unwrap() as usize - 1;
                if index < self.tabs.len() {
                    TabSwitcherAction::Select(index)
                } else {
                    TabSwitcherAction::Update
                }
            }
            // Tab key for next tab
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    // Shift+Tab = previous
                    if self.cursor > 0 {
                        self.cursor -= 1;
                    } else {
                        self.cursor = self.tabs.len().saturating_sub(1);
                    }
                } else {
                    // Tab = next
                    self.cursor = (self.cursor + 1) % self.tabs.len().max(1);
                }
                TabSwitcherAction::Update
            }
            // Backspace to delete filter char
            KeyCode::Backspace => {
                self.filter.pop();
                TabSwitcherAction::Update
            }
            // Regular character to filter
            KeyCode::Char(c) => {
                self.filter.push(c);
                self.apply_filter();
                TabSwitcherAction::Update
            }
            _ => TabSwitcherAction::Update,
        }
    }

    /// Apply filter to tabs
    fn apply_filter(&mut self) {
        if self.filter.is_empty() {
            return;
        }
        let filter_lower = self.filter.to_lowercase();
        if let Some(pos) = self
            .tabs
            .iter()
            .skip(self.cursor + 1)
            .position(|t| t.title.to_lowercase().contains(&filter_lower))
        {
            self.cursor += pos + 1;
        }
    }

    /// Get filtered tabs
    fn filtered_tabs(&self) -> Vec<(usize, &TabMetadata)> {
        if self.filter.is_empty() {
            return self.tabs.iter().enumerate().collect();
        }
        let filter_lower = self.filter.to_lowercase();
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.title.to_lowercase().contains(&filter_lower))
            .collect()
    }

    /// Render the tab switcher (internal)
    pub fn render_internal(&self, area: Rect, buf: &mut Buffer) {
        // Calculate dimensions
        let max_width = (area.width - 2).min(60);
        let max_height = (area.height - 4).min(10);

        // Draw border using Block
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ratatui::style::Color::Cyan))
            .title(" Tabs ");
        block.render(area, buf);

        // Draw tabs
        let inner_area = Rect::new(area.x + 1, area.y + 2, max_width, max_height);
        let filtered: Vec<_> = self.filtered_tabs();

        if filtered.is_empty() {
            buf.set_string(
                inner_area.x,
                inner_area.y,
                "No tabs",
                Style::default().fg(ratatui::style::Color::DarkGray),
            );
            return;
        }

        // Find cursor position in filtered list
        let _filtered_cursor = filtered
            .iter()
            .position(|(idx, _)| *idx == self.cursor)
            .unwrap_or(0);

        for (i, (tab_idx, tab)) in filtered.iter().enumerate().take(max_height as usize) {
            let y = area.y + 2 + i as u16;
            if y >= area.y + area.height - 1 {
                break;
            }

            // Highlight selected tab
            let is_selected = *tab_idx == self.cursor;

            // Tab number indicator
            let num_str = format!("{} ", tab_idx + 1);

            // Tab type indicator
            let type_str = tab.tab_type.ascii_icon();

            // Unread indicator
            let unread_indicator = if tab.unread_count > 0 {
                format!(" ({})", tab.unread_count)
            } else {
                String::new()
            };

            // Construct line
            let title_truncated = if tab.title.len() > 30 {
                format!("{}...", &tab.title[..27])
            } else {
                tab.title.clone()
            };

            let line_text = format!(
                "{}{} {}{}",
                if is_selected { ">" } else { " " },
                num_str,
                title_truncated,
                unread_indicator
            );

            let style = if is_selected {
                Style::default()
                    .fg(ratatui::style::Color::Black)
                    .bg(ratatui::style::Color::Cyan)
            } else {
                Style::default().fg(ratatui::style::Color::White)
            };

            buf.set_string(area.x + 2, y, &line_text, style);

            // Draw type indicator on the right
            let type_style = Style::default().fg(ratatui::style::Color::DarkGray);
            buf.set_string(
                area.x + area.width - 5,
                y,
                type_str,
                type_style,
            );
        }

        // Filter display at bottom
        if !self.filter.is_empty() {
            let filter_text = format!("Filter: {}", self.filter);
            let filter_style = Style::default().fg(ratatui::style::Color::Yellow);
            buf.set_string(
                area.x + 2,
                area.y + area.height - 2,
                &filter_text,
                filter_style,
            );
        }

        // Help text
        let help_text = "1-9/Enter: select  Tab: next  Esc: cancel";
        let help_style = Style::default().fg(ratatui::style::Color::DarkGray);
        buf.set_string(
            area.x + 2,
            area.y + area.height - 1,
            help_text,
            help_style,
        );
    }
}

/// Actions returned from tab switcher
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabSwitcherAction {
    /// Update the display (no action taken)
    Update,
    /// Select a tab by index
    Select(usize),
    /// Cancel without switching
    Cancel,
}

impl ModalView for TabSwitcherView {
    fn kind(&self) -> ModalKind {
        ModalKind::TabSwitcher
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match self.handle_key_internal(key) {
            TabSwitcherAction::Update => ViewAction::None,
            TabSwitcherAction::Select(idx) => {
                ViewAction::EmitAndClose(crate::tui::views::ViewEvent::TabSwitch { index: idx })
            }
            TabSwitcherAction::Cancel => ViewAction::Close,
        }
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent) -> ViewAction {
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_internal(area, buf);
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}
