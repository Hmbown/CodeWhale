//! `/feedback` picker for GitHub feedback destinations.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};

use crate::palette;
use crate::tui::views::{
    ActionHint, CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent,
    centered_modal_area, render_modal_footer, render_modal_surface,
};

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

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> ViewAction {
        match mouse.kind {
            crossterm::event::MouseEventKind::ScrollUp => self.move_up(),
            crossterm::event::MouseEventKind::ScrollDown => self.move_down(),
            _ => {}
        }
        ViewAction::None
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
        let popup_area = centered_modal_area(area, 78, (OPTIONS.len() as u16) + 7, 44, 8);

        render_modal_surface(area, popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Feedback ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("Up/Down", "move"),
                ActionHint::new("Enter", "open"),
                ActionHint::new("Esc", "cancel"),
            ],
        );

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

        Paragraph::new(lines).render(content, buf);
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

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn feedback_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        use ratatui::{buffer::Buffer, layout::Rect};
        use unicode_width::UnicodeWidthStr;

        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(FeedbackPickerView::new());
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in ["move", "open", "cancel"] {
                assert!(text.contains(label), "{w}x{h}: missing footer '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::DEEPSEEK_INK,
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
}
