//! `/mode` picker for Agent / Plan / YOLO.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette;
use crate::tui::app::AppMode;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

pub struct ModePickerView {
    cursor: usize,
}

impl ModePickerView {
    #[must_use]
    pub fn new(current: AppMode) -> Self {
        let cursor = AppMode::CHOICES
            .iter()
            .position(|mode| *mode == current)
            .unwrap_or(0);
        Self { cursor }
    }

    fn selected_mode(&self) -> AppMode {
        AppMode::CHOICES
            .get(self.cursor)
            .copied()
            .unwrap_or(AppMode::Agent)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        let max = AppMode::CHOICES.len().saturating_sub(1);
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    fn select_by_number(&mut self, number: char) -> Option<ViewAction> {
        let idx = AppMode::CHOICES
            .iter()
            .position(|mode| mode.number() == number)?;
        self.cursor = idx;
        Some(ViewAction::EmitAndClose(ViewEvent::ModeSelected {
            mode: self.selected_mode(),
        }))
    }
}

impl ModalView for ModePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ModePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter => ViewAction::EmitAndClose(ViewEvent::ModeSelected {
                mode: self.selected_mode(),
            }),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::Char(number) => self.select_by_number(number).unwrap_or(ViewAction::None),
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 68.min(area.width.saturating_sub(4)).max(44);
        let popup_height = 9.min(area.height.saturating_sub(4)).max(7);
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Mode ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Up/Down ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("move "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("select "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines = Vec::with_capacity(AppMode::CHOICES.len() + 1);
        lines.push(Line::from(Span::styled(
            "Choose how CodeWhale should operate:",
            Style::default().fg(palette::TEXT_MUTED),
        )));

        for (idx, mode) in AppMode::CHOICES.iter().copied().enumerate() {
            let is_cursor = idx == self.cursor;
            let row_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let hint_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            let pointer = if is_cursor { ">" } else { " " };

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{pointer} {}. {:<7}", mode.number(), mode.display_name()),
                    row_style,
                ),
                Span::styled(mode.picker_hint(), hint_style),
            ]));
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn opens_on_current_mode() {
        let view = ModePickerView::new(AppMode::Plan);
        assert_eq!(view.selected_mode(), AppMode::Plan);
    }

    #[test]
    fn enter_emits_selected_mode() {
        let mut view = ModePickerView::new(AppMode::Agent);
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Plan);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }
    }

    #[test]
    fn number_keys_select_modes() {
        let mut view = ModePickerView::new(AppMode::Agent);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Yolo);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }
    }
}
