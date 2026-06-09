use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::AppMode;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone, Copy)]
struct ModeRow {
    mode: AppMode,
    number: char,
}

const MODE_ROWS: &[ModeRow] = &[
    ModeRow {
        mode: AppMode::Agent,
        number: '1',
    },
    ModeRow {
        mode: AppMode::Plan,
        number: '2',
    },
    ModeRow {
        mode: AppMode::Yolo,
        number: '3',
    },
];

fn mode_hint(mode: AppMode, locale: Locale) -> &'static str {
    match mode {
        AppMode::Agent => tr(locale, MessageId::ModePickerAgentHint),
        AppMode::Plan => tr(locale, MessageId::ModePickerPlanHint),
        AppMode::Yolo => tr(locale, MessageId::ModePickerYoloHint),
    }
}

pub struct ModePickerView {
    cursor: usize,
    locale: Locale,
}

impl ModePickerView {
    #[must_use]
    pub fn new(current: AppMode, locale: Locale) -> Self {
        let cursor = MODE_ROWS
            .iter()
            .position(|row| row.mode == current)
            .unwrap_or(0);
        Self { cursor, locale }
    }

    fn selected_mode(&self) -> AppMode {
        MODE_ROWS
            .get(self.cursor)
            .map_or(AppMode::Agent, |row| row.mode)
    }

    fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        let max = MODE_ROWS.len().saturating_sub(1);
        if self.cursor < max {
            self.cursor += 1;
        }
    }

    fn select_by_number(&mut self, number: char) -> Option<ViewAction> {
        let idx = MODE_ROWS.iter().position(|row| row.number == number)?;
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
                tr(self.locale, MessageId::ModePickerTitle),
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Up/Down ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(tr(self.locale, MessageId::ModePickerFooterMove)),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(tr(self.locale, MessageId::ModePickerFooterSelect)),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw(tr(self.locale, MessageId::ModePickerFooterCancel)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines = Vec::with_capacity(MODE_ROWS.len() + 1);
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::ModePickerInstruction),
            Style::default().fg(palette::TEXT_MUTED),
        )));

        for (idx, row) in MODE_ROWS.iter().enumerate() {
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

            let name = match row.mode {
                AppMode::Agent => "Agent",
                AppMode::Plan => "Plan",
                AppMode::Yolo => "YOLO",
            };
            let hint = mode_hint(row.mode, self.locale);

            lines.push(Line::from(vec![
                Span::styled(format!("{pointer} {}. {:<7}", row.number, name), row_style),
                Span::styled(hint, hint_style),
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
        let view = ModePickerView::new(AppMode::Plan, Locale::En);
        assert_eq!(view.selected_mode(), AppMode::Plan);
    }

    #[test]
    fn enter_emits_selected_mode() {
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
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
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Yolo);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }
    }
}
