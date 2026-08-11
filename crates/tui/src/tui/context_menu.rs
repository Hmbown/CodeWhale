//! Right-click context menu for mouse-captured TUI sessions.
//!
//! v0.9.1: elevated, lightly rounded surface with leading glyphs, section
//! grouping, right-aligned key-hint chips, hover-follow, and a primary action
//! focused by default. Reduced motion opens instantly (no appear frames).

use std::cell::Cell;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::tui::menu_style;
use crate::tui::ocean;
use crate::tui::views::{ContextMenuAction, ModalKind, ModalView, ViewAction, ViewEvent};

#[derive(Debug, Clone)]
pub struct ContextMenuEntry {
    pub label: String,
    pub description: String,
    pub action: ContextMenuAction,
    /// Leading glyph / icon (e.g. "⎘", "↗", "⌥").
    pub glyph: String,
    /// Right-aligned keyboard hint chip (e.g. "↵", "y", "1").
    pub hint: String,
    /// When true, starts a new visual section above this entry.
    pub section_start: bool,
    /// Primary (most likely) action — focused by default and accent-styled.
    pub primary: bool,
}

impl ContextMenuEntry {
    pub fn new(
        label: impl Into<String>,
        description: impl Into<String>,
        action: ContextMenuAction,
    ) -> Self {
        Self {
            label: label.into(),
            description: description.into(),
            action,
            glyph: String::new(),
            hint: String::new(),
            section_start: false,
            primary: false,
        }
    }

    #[must_use]
    pub fn with_glyph(mut self, glyph: impl Into<String>) -> Self {
        self.glyph = glyph.into();
        self
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = hint.into();
        self
    }

    #[must_use]
    pub fn section_start(mut self) -> Self {
        self.section_start = true;
        self
    }

    #[must_use]
    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }
}

pub struct ContextMenuView {
    entries: Vec<ContextMenuEntry>,
    selected: usize,
    column: u16,
    row: u16,
    last_rect: Cell<Option<Rect>>,
    title: String,
    opened_at: Instant,
    reduced_motion: bool,
}

impl ContextMenuView {
    pub fn new_with_motion(
        entries: Vec<ContextMenuEntry>,
        column: u16,
        row: u16,
        title: String,
        reduced_motion: bool,
    ) -> Self {
        // Focus the primary action by default when present.
        let selected = entries.iter().position(|e| e.primary).unwrap_or(0);
        // Backfill digit hints for entries that lack one.
        let mut entries = entries;
        for (idx, entry) in entries.iter_mut().enumerate() {
            if entry.hint.is_empty() && idx < 9 {
                entry.hint = (idx + 1).to_string();
            }
            if entry.glyph.is_empty() {
                entry.glyph = default_glyph_for(&entry.action);
            }
        }
        Self {
            entries,
            selected,
            column,
            row,
            last_rect: Cell::new(None),
            title,
            opened_at: Instant::now(),
            reduced_motion,
        }
    }

    fn selected_action(&self) -> Option<ContextMenuAction> {
        self.entries
            .get(self.selected)
            .map(|entry| entry.action.clone())
    }

    fn move_selection(&mut self, delta: isize) {
        self.selected = crate::tui::list_nav::wrap_index(self.selected, self.entries.len(), delta);
    }

    fn menu_width(&self, area_width: u16) -> u16 {
        let widest = self
            .entries
            .iter()
            .map(|entry| {
                let action_width = UnicodeWidthStr::width(entry.glyph.as_str())
                    .saturating_add(UnicodeWidthStr::width(entry.label.as_str()))
                    .saturating_add(UnicodeWidthStr::width(entry.hint.as_str()))
                    .saturating_add(7);
                let detail_width =
                    UnicodeWidthStr::width(entry.description.as_str()).saturating_add(4);
                action_width.max(detail_width)
            })
            .max()
            .unwrap_or(20)
            .max(UnicodeWidthStr::width(self.title.as_str()).saturating_add(4));
        let width = u16::try_from(widest.clamp(22, 56)).unwrap_or(56);
        width.min(area_width.max(1))
    }

    fn visual_row_count(&self) -> usize {
        // title + entries + optional section dividers + a stable selected-detail row
        let dividers = self.entries.iter().filter(|e| e.section_start).count();
        let detail_rows = usize::from(self.entries.iter().any(|e| !e.description.is_empty())) * 2;
        self.entries
            .len()
            .saturating_add(1)
            .saturating_add(dividers)
            .saturating_add(detail_rows)
    }

    fn menu_rect(&self, area: Rect) -> Rect {
        let width = self.menu_width(area.width);
        let desired_height =
            u16::try_from(self.visual_row_count().saturating_add(2)).unwrap_or(u16::MAX);
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

    /// Map a mouse row to an entry index, accounting for section dividers and title.
    fn entry_at_row(&self, mouse_row: u16, rect: Rect) -> Option<usize> {
        if mouse_row <= rect.y || mouse_row >= rect.bottom().saturating_sub(1) {
            return None;
        }
        // The paragraph starts below the accent rail, and a non-empty title
        // consumes its first row before any entries are rendered.
        let mut visual = rect
            .y
            .saturating_add(1)
            .saturating_add(u16::from(!self.title.is_empty()));
        for (idx, entry) in self.entries.iter().enumerate() {
            if entry.section_start && idx > 0 {
                visual = visual.saturating_add(1);
            }
            if mouse_row == visual {
                return Some(idx);
            }
            visual = visual.saturating_add(1);
        }
        None
    }

    fn clicked_entry(&self, mouse: MouseEvent) -> Option<usize> {
        let rect = self.last_rect.get()?;
        if mouse.column < rect.x
            || mouse.column >= rect.right()
            || mouse.row < rect.y
            || mouse.row >= rect.bottom()
        {
            return None;
        }
        self.entry_at_row(mouse.row, rect)
    }

    fn appear_progress(&self) -> f32 {
        if self.reduced_motion {
            return 1.0;
        }
        let ms = self.opened_at.elapsed().as_millis() as f32;
        // Two-frame (~80 ms) soft open.
        (ms / 80.0).clamp(0.0, 1.0)
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
            MouseEventKind::Moved => {
                // Hover-follow: selection tracks the pointer over rows.
                if let Some(idx) = self.clicked_entry(mouse)
                    && self.selected != idx
                {
                    self.selected = idx;
                }
                ViewAction::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = self.clicked_entry(mouse) {
                    self.selected = idx;
                    return self.selected_action().map_or(ViewAction::Close, |action| {
                        ViewAction::EmitAndClose(ViewEvent::ContextMenuSelected { action })
                    });
                }
                // Outside click dismisses.
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

        let progress = self.appear_progress();
        let elevated = palette::SURFACE_ELEVATED;
        let shadow = ocean::mix_colors(elevated, palette::WHALE_BG, 0.35);
        let accent = palette::WHALE_INFO;
        let soft_accent = ocean::mix_colors(accent, elevated, 0.72);

        // Soft depth: paint a one-cell shadow offset below/right when space allows.
        if progress >= 1.0 && menu_area.right() < area.right() {
            for y in menu_area.y..menu_area.bottom() {
                let cell = &mut buf[(menu_area.right(), y)];
                if cell.symbol() == " " || cell.symbol().is_empty() {
                    cell.set_bg(shadow);
                }
            }
        }
        if progress >= 1.0 && menu_area.bottom() < area.bottom() {
            for x in menu_area.x..menu_area.right() {
                let cell = &mut buf[(x, menu_area.bottom())];
                if cell.symbol() == " " || cell.symbol().is_empty() {
                    cell.set_bg(shadow);
                }
            }
        }

        // Fill elevated surface (borderless form).
        for y in menu_area.y..menu_area.bottom() {
            for x in menu_area.x..menu_area.right() {
                buf[(x, y)].set_bg(elevated);
            }
        }

        // Soft top accent rail (1 cell) instead of a heavy border.
        for x in menu_area.x..menu_area.right() {
            buf[(x, menu_area.y)].set_bg(soft_accent);
        }

        let inner_width = menu_area.width.saturating_sub(2) as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();

        // Title row
        if !self.title.is_empty() {
            let title = trim_to_width(&self.title, inner_width);
            lines.push(Line::from(Span::styled(
                format!(" {title}"),
                Style::default()
                    .fg(palette::TEXT_HINT)
                    .bg(elevated)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        for (idx, entry) in self.entries.iter().enumerate() {
            if entry.section_start && idx > 0 {
                let divider = "─".repeat(inner_width.min(48));
                lines.push(Line::from(Span::styled(
                    format!(" {divider}"),
                    Style::default().fg(palette::BORDER_COLOR).bg(elevated),
                )));
            }

            let selected = idx == self.selected;
            let row_style = if selected {
                menu_style::selected_row_style()
            } else {
                let label_fg = if entry.primary {
                    accent
                } else {
                    palette::TEXT_SOFT
                };
                Style::default().fg(label_fg).bg(elevated)
            };
            let glyph = if entry.glyph.is_empty() {
                "·"
            } else {
                entry.glyph.as_str()
            };
            let hint = entry.hint.as_str();
            let glyph_width = UnicodeWidthStr::width(glyph);
            // Fixed glyph slot of two display columns: full-width icons (📌,
            // 2 cols) fit as-is, narrow ones (? / ↩, 1 col) get a trailing
            // space so every label starts at the same column.
            let glyph_slot = if glyph_width < 2 {
                format!("{glyph} ")
            } else {
                glyph.to_string()
            };
            let label_budget = inner_width
                .saturating_sub(glyph_width.max(2))
                .saturating_sub(UnicodeWidthStr::width(hint))
                .saturating_sub(4);
            let label = trim_to_width(&entry.label, label_budget);
            let pad = label_budget.saturating_sub(UnicodeWidthStr::width(label.as_str()));
            let text = format!(" {glyph_slot} {label}{} {hint} ", " ".repeat(pad));
            let style = if !selected && entry.primary {
                row_style.add_modifier(Modifier::BOLD)
            } else {
                row_style
            };
            lines.push(Line::from(Span::styled(text, style)));
        }

        if self
            .entries
            .iter()
            .any(|entry| !entry.description.is_empty())
        {
            let divider = "─".repeat(inner_width.min(48));
            lines.push(Line::from(Span::styled(
                format!(" {divider}"),
                Style::default().fg(palette::BORDER_COLOR).bg(elevated),
            )));
            let description = self
                .entries
                .get(self.selected)
                .map(|entry| trim_to_width(&entry.description, inner_width.saturating_sub(2)))
                .unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!(" {description}"),
                Style::default().fg(palette::TEXT_HINT).bg(elevated),
            )));
        }

        let body = Rect {
            x: menu_area.x,
            y: menu_area.y.saturating_add(1),
            width: menu_area.width,
            height: menu_area.height.saturating_sub(1),
        };
        Paragraph::new(lines).render(body, buf);
    }
}

/// Append git-oriented actions when a path is known (worktree manager entry).
#[must_use]
pub fn with_git_actions(
    mut entries: Vec<ContextMenuEntry>,
    path: Option<&str>,
    branch: Option<&str>,
) -> Vec<ContextMenuEntry> {
    let Some(path) = path.filter(|p| !p.is_empty()) else {
        return entries;
    };
    for (index, (label, id)) in crate::tui::worktree_manager::context_menu_git_actions(path, branch)
        .into_iter()
        .enumerate()
    {
        let action = if id == "worktrees" {
            ContextMenuAction::ExecuteCommand {
                command: "/workspace worktrees".into(),
            }
        } else if let Some(rest) = id.strip_prefix("diff:") {
            ContextMenuAction::ExecuteCommand {
                command: format!("/diff {rest}"),
            }
        } else if let Some(rest) = id.strip_prefix("open:") {
            ContextMenuAction::ExecuteCommand {
                command: format!("/open {rest}"),
            }
        } else if let Some(rest) = id.strip_prefix("branch:") {
            ContextMenuAction::CopyText {
                text: rest.to_string(),
            }
        } else {
            ContextMenuAction::CopyText {
                text: label.clone(),
            }
        };
        let entry = ContextMenuEntry::new(label, id, action).with_glyph("⌥");
        entries.push(if index == 0 {
            entry.section_start()
        } else {
            entry
        });
    }
    entries
}

fn default_glyph_for(action: &ContextMenuAction) -> String {
    // Best-effort icons from the action discriminant name.
    let name = format!("{action:?}");
    if name.contains("Copy") {
        "⎘".to_string()
    } else if name.contains("Paste") {
        "📋".to_string()
    } else if name.contains("Open") || name.contains("Edit") {
        "↗".to_string()
    } else if name.contains("Diff") || name.contains("Git") {
        "⌥".to_string()
    } else if name.contains("Select") {
        "▣".to_string()
    } else {
        "·".to_string()
    }
}

fn trim_to_width(text: &str, max_width: usize) -> String {
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
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
    use super::*;

    #[test]
    fn primary_entry_is_selected_by_default() {
        let entries = vec![
            ContextMenuEntry::new("Copy", "", ContextMenuAction::CopySelection),
            ContextMenuEntry::new("Paste", "", ContextMenuAction::Paste).primary(),
        ];
        let view = ContextMenuView::new_with_motion(entries, 0, 0, "menu".into(), false);
        assert_eq!(view.selected, 1);
    }

    #[test]
    fn reduced_motion_opens_instantly() {
        let view = ContextMenuView::new_with_motion(
            vec![ContextMenuEntry::new(
                "Copy",
                "",
                ContextMenuAction::CopySelection,
            )],
            0,
            0,
            "menu".into(),
            true,
        );
        assert!((view.appear_progress() - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn hover_rows_match_titled_menu_entries_and_dividers() {
        let entries = vec![
            ContextMenuEntry::new("Copy", "", ContextMenuAction::CopySelection),
            ContextMenuEntry::new("Paste", "", ContextMenuAction::Paste).section_start(),
            ContextMenuEntry::new("Help", "", ContextMenuAction::OpenHelp).primary(),
        ];
        let mut view = ContextMenuView::new_with_motion(entries, 0, 0, "Actions".into(), false);
        let rect = Rect::new(10, 5, 30, 10);
        view.last_rect.set(Some(rect));
        let moved = |row| MouseEvent {
            kind: MouseEventKind::Moved,
            column: rect.x.saturating_add(1),
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };

        // The title and section divider are not selectable.
        view.handle_mouse(moved(rect.y.saturating_add(1)));
        assert_eq!(view.selected, 2);
        view.handle_mouse(moved(rect.y.saturating_add(2)));
        assert_eq!(view.selected, 0);
        view.handle_mouse(moved(rect.y.saturating_add(3)));
        assert_eq!(view.selected, 0);

        view.handle_mouse(moved(rect.y.saturating_add(4)));
        assert_eq!(view.selected, 1);
        view.handle_mouse(moved(rect.y.saturating_add(5)));
        assert_eq!(view.selected, 2);
    }

    #[test]
    fn untitled_menu_entries_start_on_first_body_row() {
        let view = ContextMenuView::new_with_motion(
            vec![ContextMenuEntry::new(
                "Copy",
                "",
                ContextMenuAction::CopySelection,
            )],
            0,
            0,
            String::new(),
            false,
        );
        let rect = Rect::new(10, 5, 30, 5);

        assert_eq!(view.entry_at_row(rect.y.saturating_add(1), rect), Some(0));
    }
}
