//! Tab picker for selecting target tab in collaboration actions

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

/// Action type for tab picker
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabPickerAction {
    Delegate,
    Review,
    Meeting,
    Share,
}

impl std::fmt::Display for TabPickerAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TabPickerAction::Delegate => write!(f, "Delegate Task to"),
            TabPickerAction::Review => write!(f, "Request Review from"),
            TabPickerAction::Meeting => write!(f, "Invite to Meeting"),
            TabPickerAction::Share => write!(f, "Share Context with"),
        }
    }
}

/// Tab picker widget for selecting target tab in collaboration
pub struct TabPickerView {
    /// Available tabs (excluding current tab)
    tabs: Vec<TabMetadata>,
    /// Currently selected tab index
    cursor: usize,
    /// Action being performed
    action: TabPickerAction,
    #[allow(dead_code)]
    current_tab_id: Option<TabId>,
}

impl TabPickerView {
    /// Create a new tab picker
    pub fn new(app: &App, action: TabPickerAction) -> Self {
        let current_tab_id = app.tab_manager.active_id();

        // Filter out current tab from available tabs
        let tabs: Vec<TabMetadata> = app
            .tab_manager
            .all_tabs()
            .into_iter()
            .cloned()
            .filter(|t| Some(t.id) != current_tab_id)
            .collect();

        Self {
            tabs,
            cursor: 0,
            action,
            current_tab_id,
        }
    }

    /// Get the selected tab ID
    pub fn selected_tab_id(&self) -> Option<TabId> {
        self.tabs.get(self.cursor).map(|t| t.id)
    }
}

impl ModalView for TabPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::TabPicker
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor < self.tabs.len().saturating_sub(1) {
                    self.cursor += 1;
                }
                ViewAction::None
            }
            KeyCode::Home => {
                self.cursor = 0;
                ViewAction::None
            }
            KeyCode::End => {
                self.cursor = self.tabs.len().saturating_sub(1);
                ViewAction::None
            }
            KeyCode::Enter => {
                if let Some(tab_id) = self.selected_tab_id() {
                    ViewAction::EmitAndClose(
                        crate::tui::views::ViewEvent::CollabRequested {
                            kind: self.action,
                            to_tab: tab_id.0,
                        },
                    )
                } else {
                    ViewAction::Close
                }
            }
            KeyCode::Esc | KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ViewAction::Close
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent) -> ViewAction {
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Calculate dimensions. saturating_sub prevents underflow when the
        // terminal is shrunk below the picker's expected minimum size.
        let max_height = area.height.saturating_sub(4).min(10);

        // Draw border with title
        let title = format!(" {} ", self.action);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ratatui::style::Color::Cyan))
            .title(title);
        block.render(area, buf);

        if self.tabs.is_empty() {
            buf.set_string(
                area.x + 2,
                area.y + 2,
                "No other tabs available",
                Style::default().fg(ratatui::style::Color::DarkGray),
            );
            return;
        }

        // Draw tabs
        for (i, tab) in self.tabs.iter().enumerate().take(max_height as usize) {
            let y = area.y + 2 + i as u16;
            if y >= area.y + area.height.saturating_sub(1) {
                break;
            }

            let is_selected = i == self.cursor;

            // Tab number indicator
            let num_str = format!("{} ", i + 1);

            // Tab type indicator
            let type_str = tab.tab_type.ascii_icon();

            // Construct line
            let title_truncated = if tab.title.len() > 25 {
                format!("{}...", &tab.title[..22])
            } else {
                tab.title.clone()
            };

            let line_text = format!(
                "{}{} {}",
                if is_selected { ">" } else { " " },
                num_str,
                title_truncated,
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
                area.x + area.width.saturating_sub(5),
                y,
                type_str,
                type_style,
            );
        }

        // Help text
        let help_text = "↑↓: select  Enter: confirm  Esc: cancel";
        let help_style = Style::default().fg(ratatui::style::Color::DarkGray);
        buf.set_string(
            area.x + 2,
            area.y + area.height.saturating_sub(1),
            help_text,
            help_style,
        );
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}