//! `/statusline` multi-select picker.
//!
//! Mirrors codex-rs's `bottom_pane::status_line_setup` ergonomically: a
//! checklist of footer items the user can toggle on/off with Space (or
//! Enter), reordered by ↑/↓, applied immediately so the live footer
//! reflects every change. Enter saves to `~/.deepseek/config.toml` under
//! `tui.status_items`; Esc reverts to the snapshot taken on open.
//!
//! The picker enumerates [`StatusItem::all`] so adding a new variant in
//! `crates/tui/src/config.rs` automatically surfaces a new row here.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::config::StatusItem;
use crate::localization::truncate_to_width;
use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};
use unicode_width::UnicodeWidthStr;

const STATUS_PICKER_SELECTION_BG: ratatui::style::Color = ratatui::style::Color::Rgb(54, 72, 104);

/// Picker state. We hold both the user's working selection AND the original
/// snapshot so Esc can perfectly revert the live preview.
pub struct StatusPickerView {
    /// Every available item, in the order shown to the user. We keep this
    /// list ordered so toggles produce a stable on-screen layout that
    /// doesn't shuffle as items flip.
    rows: Vec<StatusItem>,
    /// Indices in `rows` currently checked on (the user's working set).
    selected: Vec<bool>,
    /// Highlighted row.
    cursor: usize,
    /// Snapshot of `app.status_items` at open time so Esc reverts cleanly.
    original: Vec<StatusItem>,
}

impl StatusPickerView {
    #[must_use]
    pub fn new(active: &[StatusItem]) -> Self {
        let rows: Vec<StatusItem> = StatusItem::all().to_vec();
        let selected: Vec<bool> = rows.iter().map(|item| active.contains(item)).collect();
        Self {
            rows,
            selected,
            cursor: 0,
            original: active.to_vec(),
        }
    }

    /// Build the current selection in the same order the user sees it.
    /// Preserves `StatusItem::all()` order so toggling produces deterministic
    /// `tui.status_items` output (no churn-induced diffs in config.toml).
    fn current_selection(&self) -> Vec<StatusItem> {
        self.rows
            .iter()
            .zip(self.selected.iter())
            .filter_map(|(item, on)| if *on { Some(*item) } else { None })
            .collect()
    }

    fn move_up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        if self.cursor == 0 {
            self.cursor = self.rows.len() - 1;
        } else {
            self.cursor -= 1;
        }
    }

    fn move_down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.cursor = (self.cursor + 1) % self.rows.len();
    }

    fn toggle_current(&mut self) {
        if let Some(slot) = self.selected.get_mut(self.cursor) {
            *slot = !*slot;
        }
    }

    fn live_preview_event(&self) -> ViewEvent {
        ViewEvent::StatusItemsUpdated {
            items: self.current_selection(),
            final_save: false,
        }
    }

    fn final_event(&self) -> ViewEvent {
        ViewEvent::StatusItemsUpdated {
            items: self.current_selection(),
            final_save: true,
        }
    }

    fn revert_event(&self) -> ViewEvent {
        ViewEvent::StatusItemsUpdated {
            items: self.original.clone(),
            final_save: false,
        }
    }
}

impl ModalView for StatusPickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::StatusPicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => {
                // Roll the live preview back to the snapshot so Esc means
                // "take me back to where I was."
                ViewAction::EmitAndClose(self.revert_event())
            }
            KeyCode::Enter => ViewAction::EmitAndClose(self.final_event()),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                ViewAction::None
            }
            KeyCode::Char(' ') | KeyCode::Char('x') | KeyCode::Char('X') => {
                self.toggle_current();
                ViewAction::Emit(self.live_preview_event())
            }
            KeyCode::Char('a') | KeyCode::Char('A')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                // Quality-of-life: 'a' selects all so the user can quickly
                // see every chip available before paring back.
                for slot in &mut self.selected {
                    *slot = true;
                }
                ViewAction::Emit(self.live_preview_event())
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                // 'n' clears all so the user can build up from scratch.
                for slot in &mut self.selected {
                    *slot = false;
                }
                ViewAction::Emit(self.live_preview_event())
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 64.min(area.width.saturating_sub(4)).max(40);
        // Two header lines + one row per StatusItem + one footer hint line.
        let needed_height = (self.rows.len() as u16).saturating_add(4);
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
                " Status line ",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" Space ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("toggle "),
                Span::styled(" a ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("all "),
                Span::styled(" n ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("none "),
                Span::styled(" Enter ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("save "),
                Span::styled(" Esc ", Style::default().fg(palette::TEXT_MUTED)),
                Span::raw("cancel "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let mut lines: Vec<Line> = Vec::with_capacity(self.rows.len() + 2);
        lines.push(Line::from(Span::styled(
            "Pick the chips you want in the footer:",
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));

        let visible_rows = inner.height.saturating_sub(2) as usize;
        let row_start = visible_row_start(self.rows.len(), self.cursor, visible_rows);

        for (idx, item) in self
            .rows
            .iter()
            .enumerate()
            .skip(row_start)
            .take(visible_rows)
        {
            let checked = *self.selected.get(idx).unwrap_or(&false);
            let is_cursor = idx == self.cursor;
            let mark = if checked { "[✓]" } else { "[ ]" };

            let row_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
                    .add_modifier(Modifier::BOLD)
            } else if checked {
                Style::default().fg(palette::TEXT_PRIMARY)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            let hint_style = if is_cursor {
                Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(palette::SELECTION_BG)
            } else {
                Style::default().fg(palette::TEXT_DIM)
            };
            let pointer = if is_cursor { "▸" } else { " " };

            if is_cursor {
                let selected_style = Style::default()
                    .fg(palette::SELECTION_TEXT)
                    .bg(STATUS_PICKER_SELECTION_BG)
                    .add_modifier(Modifier::BOLD);
                let line = status_row_text(pointer, mark, item, inner.width as usize);
                lines.push(Line::from(Span::styled(line, selected_style)));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(format!(" {pointer} "), row_style),
                    Span::styled(mark.to_string(), row_style),
                    Span::styled(" ", row_style),
                    Span::styled(item.label().to_string(), row_style),
                    Span::styled("  ", row_style),
                    Span::styled(format!("({})", item.hint()), hint_style),
                ]));
            }
        }

        Paragraph::new(lines).render(inner, buf);
    }
}

fn visible_row_start(total_rows: usize, cursor: usize, visible_rows: usize) -> usize {
    if total_rows == 0 || visible_rows == 0 || total_rows <= visible_rows {
        return 0;
    }
    let max_start = total_rows - visible_rows;
    cursor
        .saturating_add(1)
        .saturating_sub(visible_rows)
        .min(max_start)
}

fn status_row_text(pointer: &str, mark: &str, item: &StatusItem, width: usize) -> String {
    let text = format!(" {pointer} {mark} {}  ({})", item.label(), item.hint());
    let mut text = truncate_to_width(&text, width);
    let current_width = text.width();
    if current_width < width {
        text.push_str(&" ".repeat(width - current_width));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_with_active_items_pre_selected() {
        let active = StatusItem::default_footer();
        let view = StatusPickerView::new(&active);
        assert_eq!(view.current_selection(), active);
    }

    #[test]
    fn space_toggles_current_row_and_emits_live_preview() {
        let active = StatusItem::default_footer();
        let mut view = StatusPickerView::new(&active);
        // Cursor starts at row 0 = StatusItem::Mode (currently checked).
        let action = view.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        match action {
            ViewAction::Emit(ViewEvent::StatusItemsUpdated { items, final_save }) => {
                assert!(!final_save);
                assert!(!items.contains(&StatusItem::Mode));
            }
            other => panic!("expected live preview emit, got {other:?}"),
        }
    }

    #[test]
    fn enter_emits_final_save() {
        let active = StatusItem::default_footer();
        let mut view = StatusPickerView::new(&active);
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::StatusItemsUpdated { final_save, .. }) => {
                assert!(final_save);
            }
            other => panic!("expected final save EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn esc_reverts_to_snapshot() {
        let active = StatusItem::default_footer();
        let mut view = StatusPickerView::new(&active);
        // Toggle a few items off so the working set diverges from snapshot.
        view.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        view.move_down();
        view.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        match action {
            ViewAction::EmitAndClose(ViewEvent::StatusItemsUpdated { items, final_save }) => {
                assert!(!final_save);
                assert_eq!(items, active);
            }
            other => panic!("expected revert EmitAndClose, got {other:?}"),
        }
    }

    #[test]
    fn select_all_and_select_none_keys_work() {
        let active: Vec<StatusItem> = Vec::new();
        let mut view = StatusPickerView::new(&active);
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        match action {
            ViewAction::Emit(ViewEvent::StatusItemsUpdated { items, .. }) => {
                assert_eq!(items.len(), StatusItem::all().len());
            }
            other => panic!("expected select-all emit, got {other:?}"),
        }
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE));
        match action {
            ViewAction::Emit(ViewEvent::StatusItemsUpdated { items, .. }) => {
                assert!(items.is_empty());
            }
            other => panic!("expected select-none emit, got {other:?}"),
        }
    }

    #[test]
    fn arrow_keys_wrap_cursor_at_edges() {
        let active = StatusItem::default_footer();
        let mut view = StatusPickerView::new(&active);
        assert_eq!(view.cursor, 0);
        view.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(view.cursor, StatusItem::all().len() - 1);
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(view.cursor, 0);
        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(view.cursor, 1);
        view.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(view.cursor, 0);
    }

    #[test]
    fn visible_row_start_keeps_cursor_in_view() {
        assert_eq!(visible_row_start(14, 0, 8), 0);
        assert_eq!(visible_row_start(14, 7, 8), 0);
        assert_eq!(visible_row_start(14, 8, 8), 1);
        assert_eq!(visible_row_start(14, 13, 8), 6);
    }

    #[test]
    fn selected_row_text_fills_available_width() {
        let text = status_row_text("▸", "[ ]", &StatusItem::LastToolElapsed, 40);
        assert_eq!(text.width(), 40);
        assert!(text.starts_with(" ▸ [ ] Last tool elapsed"));
    }
}
