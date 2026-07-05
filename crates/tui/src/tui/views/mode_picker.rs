//! `/mode` picker for Agent / Plan / YOLO.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::AppMode;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface,
};

pub struct ModePickerView {
    cursor: usize,
    locale: Locale,
    /// Content rect of the last render, for mouse hit-testing. Row 0 is the
    /// prompt line; choice `idx` renders on row `1 + idx`.
    last_content: std::cell::Cell<Option<Rect>>,
}

impl ModePickerView {
    #[must_use]
    pub fn new(current: AppMode, locale: Locale) -> Self {
        let cursor = AppMode::CHOICES
            .iter()
            .position(|mode| *mode == current)
            .unwrap_or(0);
        Self {
            cursor,
            locale,
            last_content: std::cell::Cell::new(None),
        }
    }

    /// Map a mouse position to a choice index using the last rendered
    /// content rect.
    fn choice_at(&self, column: u16, row: u16) -> Option<usize> {
        let content = self.last_content.get()?;
        if column < content.x
            || column >= content.x.saturating_add(content.width)
            || row <= content.y
            || row >= content.y.saturating_add(content.height)
        {
            return None;
        }
        let idx = usize::from(row - content.y - 1);
        (idx < AppMode::CHOICES.len()).then_some(idx)
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

    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> ViewAction {
        use crossterm::event::{MouseButton, MouseEventKind};
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_up(),
            MouseEventKind::ScrollDown => self.move_down(),
            MouseEventKind::Down(MouseButton::Left) => {
                // Click selects a row; clicking the already-selected row
                // activates it (mirrors Enter).
                if let Some(idx) = self.choice_at(mouse.column, mouse.row) {
                    if idx == self.cursor {
                        return ViewAction::EmitAndClose(ViewEvent::ModeSelected {
                            mode: self.selected_mode(),
                        });
                    }
                    self.cursor = idx;
                }
            }
            _ => {}
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_height = u16::try_from(AppMode::CHOICES.len()).unwrap_or(3) + 7;
        let popup_area = centered_modal_area(area, 68, popup_height, 44, 8);

        render_modal_surface(area, popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Mode ",
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
                ActionHint::new("↑/↓", "move"),
                ActionHint::new("Enter", "select"),
                ActionHint::new("Esc", "cancel"),
            ],
        );
        self.last_content.set(Some(content));

        let mut lines = Vec::with_capacity(AppMode::CHOICES.len() + 1);
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::ModePickerPrompt),
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
            let name = mode.display_name_localized(self.locale);
            // Pad by terminal columns, not scalar count, so wide (CJK) mode
            // names keep the hint column aligned.
            let pad = " ".repeat(7usize.saturating_sub(UnicodeWidthStr::width(&*name)));

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{pointer} {}. {name}{pad}", mode.number()),
                    row_style,
                ),
                Span::styled(mode.picker_hint_localized(self.locale), hint_style),
            ]));
        }

        Paragraph::new(lines).render(content, buf);
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

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    fn render_at(width: u16, height: u16) -> (Buffer, Rect) {
        use crate::tui::views::ViewStack;
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        // Pre-fill with a sentinel so any cell the composited modal fails to
        // paint (bleed-through) is detectable as a surviving 'X'.
        for y in 0..height {
            for x in 0..width {
                buf[(x, y)].set_symbol("X");
            }
        }
        // Render through the ViewStack so the shared opaque backdrop is painted
        // exactly as it is in production.
        let mut stack = ViewStack::new();
        stack.push(ModePickerView::new(AppMode::Agent, Locale::En));
        stack.render(area, &mut buf);
        (buf, area)
    }

    fn rows(buf: &Buffer, area: Rect) -> Vec<String> {
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn mode_picker_is_usable_and_opaque_at_blocker_sizes() {
        for (w, h) in BLOCKER_SIZES {
            let (buf, area) = render_at(w, h);
            let text = rows(&buf, area).join("\n");

            // Action labels are present (footer never drops an action).
            assert!(text.contains("move"), "{w}x{h}: missing 'move' hint");
            assert!(text.contains("select"), "{w}x{h}: missing 'select' hint");
            assert!(text.contains("cancel"), "{w}x{h}: missing 'cancel' hint");

            // Composited frame is fully opaque: no sentinel survives and every
            // cell carries the modal/backdrop ink background.
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            let center = &buf[(w / 2, h / 2)];
            assert_eq!(
                center.bg,
                palette::DEEPSEEK_INK,
                "{w}x{h}: modal interior must be opaque"
            );

            // No row exceeds the frame width (no horizontal overflow).
            for (y, row) in rows(&buf, area).iter().enumerate() {
                assert!(
                    UnicodeWidthStr::width(row.trim_end()) <= w as usize,
                    "{w}x{h}: row {y} overflows width: {row:?}"
                );
            }
        }
    }

    #[test]
    fn number_keys_select_modes() {
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));

        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Yolo);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }
    }
}
