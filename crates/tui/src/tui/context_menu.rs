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
                        .fg(palette::TEXT_PRIMARY)
                        .bg(palette::DEEPSEEK_BLUE)
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
            .border_style(Style::default().fg(palette::DEEPSEEK_SKY))
            .style(Style::default().bg(palette::SURFACE_ELEVATED))
            .padding(Padding::horizontal(0));

        Paragraph::new(lines).block(block).render(menu_area, buf);
    }
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return text.chars().take(max_width).collect();
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
}
