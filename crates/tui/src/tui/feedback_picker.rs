//! `/feedback` picker for GitHub feedback destinations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette;
use crate::tui::views::{CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone, Copy)]
struct FeedbackOption {
    number: char,
    label: &'static str,
    description: &'static str,
    command: &'static str,
}

const OPTIONS: &[FeedbackOption] = &[
    FeedbackOption {
        number: '1',
        label: "Bug report",
        description: "Report a problem or regression",
        command: "/feedback bug",
    },
    FeedbackOption {
        number: '2',
        label: "Feature request",
        description: "Suggest an idea or improvement",
        command: "/feedback feature",
    },
    FeedbackOption {
        number: '3',
        label: "Security vulnerability",
        description: "Review the security policy before reporting",
        command: "/feedback security",
    },
];

pub struct FeedbackPickerView {
    selected: usize,
}

impl FeedbackPickerView {
    #[must_use]
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        let max = OPTIONS.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }

    fn select_number(&mut self, number: char) -> Option<ViewAction> {
        let idx = OPTIONS.iter().position(|option| option.number == number)?;
        self.selected = idx;
        Some(self.selected_action())
    }

    fn selected_action(&self) -> ViewAction {
        let command = OPTIONS
            .get(self.selected)
            .map(|option| option.command)
            .unwrap_or(OPTIONS[0].command)
            .to_string();
        ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
            action: CommandPaletteAction::ExecuteCommand { command },
        })
    }
}

impl Default for FeedbackPickerView {
    fn default() -> Self {
        Self::new()
    }
}

impl ModalView for FeedbackPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::FeedbackPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Enter => self.selected_action(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::Char(number)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && OPTIONS.iter().any(|option| option.number == number) =>
            {
                self.select_number(number).unwrap_or(ViewAction::None)
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 78.min(area.width.saturating_sub(4)).max(44);
        let needed_height = (OPTIONS.len() as u16).saturating_add(7);
        let popup_height = needed_height.min(area.height.saturating_sub(4)).max(8);

        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Feedback ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Up/Down ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("move "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("open "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines = Vec::with_capacity(OPTIONS.len() + 2);
        lines.push(Line::from(Span::styled(
            "Choose where to send feedback:",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));

        for (idx, option) in OPTIONS.iter().enumerate() {
            let is_selected = idx == self.selected;
            let row_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let desc_style = if is_selected {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            let pointer = if is_selected { ">" } else { " " };

            lines.push(Line::from(vec![
                Span::styled(format!(" {pointer} {}. ", option.number), row_style),
                Span::styled(option.label, row_style),
                Span::raw("    "),
                Span::styled(option.description, desc_style),
            ]));
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emitted_command(action: ViewAction) -> String {
        match action {
            ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                action: CommandPaletteAction::ExecuteCommand { command },
            }) => command,
            other => panic!("expected feedback command emit, got {other:?}"),
        }
    }

    #[test]
    fn enter_emits_selected_feedback_command() {
        let mut view = FeedbackPickerView::new();
        let command =
            emitted_command(view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(command, "/feedback bug");
    }

    #[test]
    fn arrow_down_selects_feature_command() {
        let mut view = FeedbackPickerView::new();
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let command =
            emitted_command(view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert_eq!(command, "/feedback feature");
    }

    #[test]
    fn digit_selects_security_command() {
        let mut view = FeedbackPickerView::new();
        let command =
            emitted_command(view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE)));
        assert_eq!(command, "/feedback security");
    }

    #[test]
    fn esc_closes_picker() {
        let mut view = FeedbackPickerView::new();
        assert!(matches!(
            view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ViewAction::Close
        ));
    }
}
