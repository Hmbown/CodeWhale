//! `/mode` picker for Act / Plan / Operate.
//!
//! Dual-path list (Lane C start): keyboard ↑/↓/Enter and mouse hover/click
//! share the same selected row via a small [`HitMap`].

use std::cell::RefCell;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::Locale;
use crate::palette;
use crate::tui::app::AppMode;
use crate::tui::hit_region::HitMap;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_chrome,
};

pub struct ModePickerView {
    cursor: usize,
    locale: Locale,
    /// Row hit regions from the last paint (shared by hover + click).
    hit_map: RefCell<HitMap>,
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
            hit_map: RefCell::new(HitMap::new()),
        }
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

    /// Align keyboard focus with the row under the pointer (hover = select).
    fn focus_row_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.hit_map.borrow().row_at(column, row) else {
            return false;
        };
        if index < AppMode::CHOICES.len() && self.cursor != index {
            self.cursor = index;
            return true;
        }
        index < AppMode::CHOICES.len()
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            // Hover shares keyboard focus: the highlighted row is the same
            // one Enter would activate.
            MouseEventKind::Moved | MouseEventKind::Drag(MouseButton::Left) => {
                let _ = self.focus_row_at(mouse.column, mouse.row);
                ViewAction::None
            }
            // Click selects the same row Enter would, and activates it
            // (mode picker is pick-and-close).
            MouseEventKind::Down(MouseButton::Left) => {
                if self.focus_row_at(mouse.column, mouse.row) {
                    ViewAction::EmitAndClose(ViewEvent::ModeSelected {
                        mode: self.selected_mode(),
                    })
                } else {
                    ViewAction::None
                }
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_height = u16::try_from(AppMode::CHOICES.len()).unwrap_or(3) + 7;

        // Shared modal chrome recipe (COH-05): centered opaque surface, titled
        // border, and action-hint footer — one call replaces the hand-rolled
        // block + surface + footer that lived here before the recipe existed.
        let chrome = render_modal_chrome(
            area,
            buf,
            Line::from(Span::styled(
                " Mode ",
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )),
            68,
            popup_height,
            44,
            8,
            &[
                ActionHint::new("↑/↓", "move"),
                ActionHint::new("Enter", "select"),
                ActionHint::new("Esc", "cancel"),
            ],
        );
        let content = chrome.body;

        let mut lines = Vec::with_capacity(AppMode::CHOICES.len());
        let mut map = HitMap::new();

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

            // Row y = content origin + line index (one mode per line).
            let row_y = content
                .y
                .saturating_add(u16::try_from(idx).unwrap_or(u16::MAX));
            if row_y < content.y.saturating_add(content.height) {
                map.push_row(idx, content.x, row_y, content.width);
            }

            lines.push(Line::from(vec![
                Span::styled(
                    format!("{pointer} {}. {name}{pad}", mode.number()),
                    row_style,
                ),
                Span::styled(mode.picker_hint_localized(self.locale), hint_style),
            ]));
        }

        *self.hit_map.borrow_mut() = map;
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
                palette::WHALE_BG,
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
        // Visible roster: 1 Act, 2 Plan, 3 Operate. No Multitask / YOLO / gap.
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Operate);
            }
            other => panic!("expected ModeSelected, got {other:?}"),
        }

        // Legacy YOLO shorthand (4) is not offered by the picker.
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));

        // Old Operate number (5) is gone — no numeric gaps.
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::None));
    }

    #[test]
    fn mouse_hover_and_click_share_keyboard_selected_row() {
        use crate::tui::views::ViewStack;

        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        // Act is index 0 by default; paint so hit map is populated.
        view.render(area, &mut buf);
        assert!(
            !view.hit_map.borrow().is_empty(),
            "render must fill hit map"
        );
        assert_eq!(view.hit_map.borrow().len(), AppMode::CHOICES.len());

        // Grab Plan row (index 1) coordinates from the hit map.
        let plan_hit = view
            .hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|r| {
                matches!(
                    r.target,
                    crate::tui::hit_region::HitTarget::Row { index: 1 }
                )
            })
            .copied()
            .expect("plan row hit region");
        let col = plan_hit.rect.x + 2;
        let row = plan_hit.rect.y;

        // Hover moves keyboard focus without activating.
        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.cursor, 1, "hover should focus Plan row");
        assert_eq!(view.selected_mode(), AppMode::Plan);

        // Enter on the hovered row matches keyboard Enter path.
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Plan);
            }
            other => panic!("expected ModeSelected Plan via Enter after hover, got {other:?}"),
        }

        // Fresh view: click activates the row under the pointer (same as Enter).
        let mut view = ModePickerView::new(AppMode::Agent, Locale::En);
        view.render(area, &mut buf);
        let operate_hit = view
            .hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|r| {
                matches!(
                    r.target,
                    crate::tui::hit_region::HitTarget::Row { index: 2 }
                )
            })
            .copied()
            .expect("operate row hit region");
        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: operate_hit.rect.x + 2,
            row: operate_hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        match action {
            ViewAction::EmitAndClose(ViewEvent::ModeSelected { mode }) => {
                assert_eq!(mode, AppMode::Operate);
            }
            other => panic!("expected ModeSelected Operate via click, got {other:?}"),
        }

        // Stack still composites without panic when mouse is routed.
        let mut stack = ViewStack::new();
        stack.push(ModePickerView::new(AppMode::Plan, Locale::En));
        stack.render(area, &mut buf);
    }
}
