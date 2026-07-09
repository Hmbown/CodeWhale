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
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};

use crate::config::{ApiProvider, StatusItem};
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer, render_modal_surface,
};
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
    locale: Locale,
}

impl StatusPickerView {
    #[must_use]
    pub fn new(active: &[StatusItem], provider: ApiProvider, locale: Locale) -> Self {
        let rows: Vec<StatusItem> = StatusItem::all()
            .iter()
            .filter(|item| item.is_available_for(provider))
            .copied()
            .collect();
        let selected: Vec<bool> = rows.iter().map(|item| active.contains(item)).collect();
        Self {
            rows,
            selected,
            cursor: 0,
            original: active.to_vec(),
            locale,
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
        // Two header lines + one row per StatusItem + the wrapping action
        // footer that now lives inside the body (one row more than the old
        // border footer). centered_modal_area clamps this to the frame and
        // lets the scroll offset absorb any remaining overflow.
        let needed_height = (self.rows.len() as u16).saturating_add(5);
        let popup_area = centered_modal_area(area, 64, needed_height, 40, 8);

        render_modal_surface(area, popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                tr(self.locale, MessageId::StatusPickerTitle),
                Style::default()
                    .fg(palette::WHALE_INFO)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::WHALE_BG))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new(
                    "Space",
                    tr(self.locale, MessageId::StatusPickerActionToggle),
                ),
                ActionHint::new("a", tr(self.locale, MessageId::StatusPickerActionAll)),
                ActionHint::new("n", tr(self.locale, MessageId::StatusPickerActionNone)),
                ActionHint::new("Enter", tr(self.locale, MessageId::StatusPickerActionSave)),
                ActionHint::new("Esc", tr(self.locale, MessageId::StatusPickerActionCancel)),
            ],
        );

        let visible_rows = content.height.saturating_sub(2) as usize;
        let row_start = visible_row_start(self.rows.len(), self.cursor, visible_rows);

        let mut lines: Vec<Line> = Vec::with_capacity(visible_rows + 2);
        lines.push(Line::from(Span::styled(
            tr(self.locale, MessageId::StatusPickerInstruction),
            Style::default().fg(palette::TEXT_MUTED),
        )));
        lines.push(Line::from(""));

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
                let line = status_row_text(pointer, mark, item, content.width as usize);
                lines.push(Line::from(Span::styled(line, selected_style)));
            } else {
                let label = item.label();
                let hint = item.hint();
                let prefix = format!(" {pointer} {mark} {label}  (");
                let truncated_hint = crate::tui::ui_text::semantic_truncate_between_affixes(
                    &prefix,
                    hint,
                    ")",
                    usize::from(content.width),
                );
                lines.push(Line::from(vec![
                    Span::styled(format!(" {pointer} "), row_style),
                    Span::styled(mark.to_string(), row_style),
                    Span::styled(" ", row_style),
                    Span::styled(label.to_string(), row_style),
                    Span::styled("  ", row_style),
                    Span::styled(format!("({})", truncated_hint), hint_style),
                ]));
            }
        }

        Paragraph::new(lines).render(content, buf);
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
    let prefix = format!(" {pointer} {mark} {}  (", item.label());
    let mut text =
        crate::tui::ui_text::semantic_truncate_with_affixes(&prefix, item.hint(), ")", width);
    let current_width = text.width();
    if current_width < width {
        text.push_str(&" ".repeat(width - current_width));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localization::Locale;

    #[test]
    fn opens_with_active_items_pre_selected() {
        let active = StatusItem::default_footer();
        let view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
        assert_eq!(view.current_selection(), active);
    }

    #[test]
    fn space_toggles_current_row_and_emits_live_preview() {
        let active = StatusItem::default_footer();
        let mut view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
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
        let mut view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
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
        let mut view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
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
        let mut view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
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
        let mut view = StatusPickerView::new(&active, ApiProvider::Deepseek, Locale::En);
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

    #[test]
    fn selected_row_text_semantically_truncates_hint_at_narrow_width() {
        let text = status_row_text("▸", "[ ]", &StatusItem::LastToolElapsed, 49);
        assert_eq!(text.width(), 49);
        assert!(text.contains("ms of the most…"), "{text:?}");
        assert!(!text.contains("ms of the most r"), "{text:?}");
    }

    #[test]
    fn balance_excluded_for_non_deepseek_provider() {
        let active = StatusItem::default_footer();
        let view = StatusPickerView::new(&active, ApiProvider::Openrouter, Locale::En);
        assert!(!view.rows.contains(&StatusItem::Balance));
        assert!(view.rows.contains(&StatusItem::Mode));
    }

    #[test]
    fn status_picker_displays_localized_title_for_zh_hans() {
        assert_eq!(tr(Locale::ZhHans, MessageId::StatusPickerTitle), " 状态行 ");
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn status_picker_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        let active = StatusItem::default_footer();
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(StatusPickerView::new(
                &active,
                ApiProvider::Deepseek,
                Locale::En,
            ));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in ["toggle", "all", "none", "save", "cancel"] {
                assert!(text.contains(label), "{w}x{h}: missing footer '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            assert_eq!(
                buf[(w / 2, h / 2)].bg,
                palette::WHALE_BG,
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

    #[test]
    fn status_picker_no_english_leak_in_non_en_locales() {
        for locale in [
            Locale::Ja,
            Locale::ZhHans,
            Locale::ZhHant,
            Locale::PtBr,
            Locale::Es419,
            Locale::Vi,
        ] {
            let title = tr(locale, MessageId::StatusPickerTitle);
            assert!(
                !title.contains("Status"),
                "{} leaks English in title: {title}",
                locale.tag()
            );
            let instruction = tr(locale, MessageId::StatusPickerInstruction);
            assert!(
                !instruction.contains("footer"),
                "{} leaks English in instruction: {instruction}",
                locale.tag()
            );
        }
    }
}
