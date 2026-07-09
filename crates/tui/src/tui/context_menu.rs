//! Right-click context menu for mouse-captured TUI sessions.

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::tui::views::{ContextMenuAction, ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone)]
pub struct ContextMenuEntry {
    pub label: String,
    pub description: String,
    pub action: ContextMenuAction,
}

pub struct ContextMenuView {
    entries: Vec<ContextMenuEntry>,
    selected: usize,
    column: u16,
    row: u16,
    last_rect: Cell<Option<Rect>>,
    title: String,
}

impl ContextMenuView {
    pub fn new(entries: Vec<ContextMenuEntry>, column: u16, row: u16, title: String) -> Self {
        Self {
            entries,
            selected: 0,
            column,
            row,
            last_rect: Cell::new(None),
            title,
        }
    }

    fn selected_action(&self) -> Option<ContextMenuAction> {
        self.entries
            .get(self.selected)
            .map(|entry| entry.action.clone())
    }

    fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            self.selected = 0;
            return;
        }
        let max = self.entries.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    fn menu_width(&self, area_width: u16) -> u16 {
        let widest = self
            .entries
            .iter()
            .map(|entry| {
                UnicodeWidthStr::width(entry.label.as_str())
                    + UnicodeWidthStr::width(entry.description.as_str())
                    + 8
            })
            .max()
            .unwrap_or(20);
        let width = u16::try_from(widest.clamp(24, 64)).unwrap_or(64);
        width.min(area_width.max(1))
    }

    fn menu_rect(&self, area: Rect) -> Rect {
        let width = self.menu_width(area.width);
        let desired_height =
            u16::try_from(self.entries.len().saturating_add(2)).unwrap_or(u16::MAX);
        let height = desired_height.min(area.height.max(1));
        let max_x = area.right().saturating_sub(width).max(area.x);
        let max_y = area.bottom().saturating_sub(height).max(area.y);
        let x = self.column.max(area.x).min(max_x);
        let y = self.row.max(area.y).min(max_y);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn clicked_entry(&self, mouse: MouseEvent) -> Option<usize> {
        let rect = self.last_rect.get()?;
        if mouse.column <= rect.x
            || mouse.column >= rect.right().saturating_sub(1)
            || mouse.row <= rect.y
            || mouse.row >= rect.bottom().saturating_sub(1)
        {
            return None;
        }
        let idx = mouse.row.saturating_sub(rect.y + 1) as usize;
        (idx < self.entries.len()).then_some(idx)
    }
}

impl ModalView for ContextMenuView {
    fn kind(&self) -> ModalKind {
        ModalKind::ContextMenu
    }

    /// The context menu is a small anchored popup, not a full-screen modal:
    /// scope the central backdrop to the menu itself so opening it does not
    /// blank the transcript behind it (#3868).
    fn occupied_region(&self, area: Rect) -> Rect {
        self.menu_rect(area)
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::Enter => self.selected_action().map_or(ViewAction::Close, |action| {
                ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected { action })
            }),
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = c.to_digit(10).and_then(|digit| {
                    let digit = usize::try_from(digit).ok()?;
                    digit.checked_sub(1)
                });
                if let Some(idx) = idx.filter(|idx| *idx < self.entries.len()) {
                    self.selected = idx;
                    return self.selected_action().map_or(ViewAction::Close, |action| {
                        ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected { action })
                    });
                }
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.clicked_entry(mouse) {
                    self.selected = idx;
                    return self.selected_action().map_or(ViewAction::Close, |action| {
                        ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected { action })
                    });
                }
                ViewAction::Close
            }
            MouseEventKind::Down(MouseButton::Right) => ViewAction::Close,
            MouseEventKind::ScrollUp => {
                self.move_selection(-1);
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.move_selection(1);
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let menu_area = self.menu_rect(area);
        self.last_rect.set(Some(menu_area));
        Clear.render(menu_area, buf);

        let inner_width = menu_area.width.saturating_sub(2) as usize;
        let lines = self
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                let label = format!("{} {}", idx + 1, entry.label);
                let description = if entry.description.trim().is_empty() {
                    String::new()
                } else {
                    format!(" - {}", entry.description)
                };
                let text = trim_to_width(&format!("{label}{description}"), inner_width);
                let style = if idx == self.selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(palette::TEXT_SOFT)
                        .bg(palette::SURFACE_ELEVATED)
                };
                Line::from(Span::styled(text, style))
            })
            .collect::<Vec<_>>();

        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::WHALE_INFO))
            .style(Style::default().bg(palette::SURFACE_ELEVATED))
            .padding(Padding::horizontal(0));

        Paragraph::new(lines).block(block).render(menu_area, buf);
    }
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    // #3488: the narrow-budget branch must accumulate *display* width, not char
    // count. The previous `chars().take(max_width)` truncated by character count,
    // which overflowed the column budget for wide (CJK) glyphs — three Han chars
    // are six columns but `take(3)` kept all three, corrupting the menu border.
    if max_width <= 3 {
        let mut out = String::new();
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > max_width {
                break;
            }
            out.push(ch);
            width += ch_width;
        }
        return out;
    }

    let limit = max_width.saturating_sub(3);
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;
    use ratatui::buffer::Buffer;

    use super::*;

    fn entry(label: &str, action: ContextMenuAction) -> ContextMenuEntry {
        ContextMenuEntry {
            label: label.to_string(),
            description: String::new(),
            action,
        }
    }

    #[test]
    fn enter_emits_selected_action() {
        let mut view = ContextMenuView::new(
            vec![
                entry("Paste", ContextMenuAction::Paste),
                entry("Help", ContextMenuAction::OpenHelp),
            ],
            5,
            5,
            " Right click ".to_string(),
        );

        view.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let action = view.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected {
                action: ContextMenuAction::OpenHelp
            })
        ));
    }

    #[test]
    fn occupied_region_covers_only_the_menu_popup() {
        // Regression test for #3868: the central modal backdrop clears each
        // view's occupied_region. If the context menu reports the whole
        // frame, right-clicking blanks the entire UI behind the small menu.
        let view = ContextMenuView::new(
            vec![
                entry("Paste", ContextMenuAction::Paste),
                entry("Help", ContextMenuAction::OpenHelp),
            ],
            10,
            5,
            " Right click ".to_string(),
        );
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };

        let region = ModalView::occupied_region(&view, area);

        assert_eq!(region, view.menu_rect(area));
        assert!(region.width < area.width);
        assert!(region.height < area.height);
    }

    #[test]
    fn menu_clamps_to_render_area() {
        let view = ContextMenuView::new(
            vec![entry("Paste", ContextMenuAction::Paste)],
            200,
            80,
            " Right click ".to_string(),
        );

        let rect = view.menu_rect(Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        });

        assert!(rect.right() <= 40);
        assert!(rect.bottom() <= 10);
    }

    #[test]
    fn left_click_selects_rendered_entry() {
        let mut view = ContextMenuView::new(
            vec![
                entry("Paste", ContextMenuAction::Paste),
                entry("Help", ContextMenuAction::OpenHelp),
            ],
            2,
            2,
            " Right click ".to_string(),
        );
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 10,
        };
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let action = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        });

        assert!(matches!(
            action,
            ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected {
                action: ContextMenuAction::OpenHelp
            })
        ));
    }

    // --- Unicode / CJK / terminal-width QA (issue #3488) -------------------
    // The context menu is a bordered selector: every entry is clipped by
    // `trim_to_width` to the menu's inner width so wide glyphs never push past
    // the border. These exercise that same clipping path the renderer uses.

    fn entry_with_description(
        label: &str,
        description: &str,
        action: ContextMenuAction,
    ) -> ContextMenuEntry {
        ContextMenuEntry {
            label: label.to_string(),
            description: description.to_string(),
            action,
        }
    }

    /// Concatenate the rendered cells of one menu entry row (between the left
    /// and right borders) so its display width can be measured.
    fn rendered_entry_row(buf: &Buffer, rect: Rect, entry_index: usize) -> String {
        let y = rect.y + 1 + entry_index as u16;
        let mut out = String::new();
        for x in (rect.x + 1)..rect.right().saturating_sub(1) {
            out.push_str(buf[(x, y)].symbol());
        }
        out
    }

    #[test]
    fn trim_to_width_truncates_cjk_by_display_columns_not_char_count() {
        // Regression for #3488: the narrow-budget branch used to take
        // `max_width` *characters*, which overflowed the column budget for CJK
        // (three Han glyphs = six columns, but `take(3)` kept all three and blew
        // past a three-column budget, corrupting the menu border). Each Han
        // glyph is two columns, so a budget of three fits exactly one glyph.
        let out = trim_to_width("中文项目", 3);
        assert_eq!(out, "中");
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 2);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 3);
        assert!(!out.contains('\u{FFFD}'), "truncation split a wide glyph");

        // Budgets 1, 2, 3 all stay within bounds by display width.
        for budget in [1usize, 2, 3] {
            let out = trim_to_width("中文项目标题", budget);
            assert!(
                UnicodeWidthStr::width(out.as_str()) <= budget,
                "budget {budget}: {out:?} overflowed"
            );
            assert!(!out.contains('\u{FFFD}'), "budget {budget}: split a glyph");
        }
    }

    #[test]
    fn trim_to_width_keeps_combining_marks_and_emoji_within_budget() {
        // Combining mark (U+0301) is zero-width, so "café" stays 4 columns.
        let out = trim_to_width("cafe\u{0301} extra", 4);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 4);
        assert!(!out.contains('\u{FFFD}'));
        // Emoji is two columns; the ellipsis branch keeps the budget.
        let out = trim_to_width("\u{1F433} \u{1F433} \u{1F433} whales", 6);
        assert!(UnicodeWidthStr::width(out.as_str()) <= 6);
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn context_menu_cjk_entries_render_within_borders_at_narrow_and_medium_widths() {
        // A selector with CJK labels and mixed-width descriptions — exactly the
        // "CJK next to ASCII metadata" row the issue tracks. Rendered into a
        // bordered menu, no entry row may overflow the inner width (which would
        // corrupt the right border) or emit a replacement char.
        let entries = vec![
            entry_with_description("粘贴", "insert from clipboard", ContextMenuAction::Paste),
            entry_with_description("复制", "copy selection", ContextMenuAction::OpenHelp),
            entry_with_description(
                "搜索 🔍",
                "mixed ascii + emoji + cjk",
                ContextMenuAction::Paste,
            ),
        ];

        for width in [30u16, 80] {
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height: 24,
            };
            let view = ContextMenuView::new(entries.clone(), 4, 4, " 右键菜单 ".to_string());
            let mut buf = Buffer::empty(area);
            view.render(area, &mut buf);

            let rect = view.menu_rect(area);
            let inner_width = rect.width.saturating_sub(2) as usize;

            for (idx, entry) in entries.iter().enumerate() {
                let label = format!("{} {}", idx + 1, entry.label);
                let description = if entry.description.trim().is_empty() {
                    String::new()
                } else {
                    format!(" - {}", entry.description)
                };
                let clipped = trim_to_width(&format!("{label}{description}"), inner_width);
                assert!(
                    UnicodeWidthStr::width(clipped.as_str()) <= inner_width,
                    "width={width} entry {idx} clipped text overflows inner width {inner_width}: {clipped:?}"
                );

                let row = rendered_entry_row(&buf, rect, idx);
                assert!(
                    !row.contains('\u{FFFD}'),
                    "width={width} entry {idx} rendered a split glyph: {row:?}"
                );
                assert_eq!(
                    buf[(rect.right().saturating_sub(1), rect.y + 1 + idx as u16)].symbol(),
                    "│",
                    "width={width} entry {idx} corrupted the right border: {row:?}"
                );
            }

            // The selected (first) entry's CJK label must actually appear in the
            // buffer, proving the content was not dropped by truncation.
            let first = rendered_entry_row(&buf, rect, 0);
            assert!(
                first.contains('粘'),
                "width={width}: CJK label dropped: {first:?}"
            );
        }
    }
}
