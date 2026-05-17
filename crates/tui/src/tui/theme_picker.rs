//! `/theme` picker with live preview.
//!
//! Modeled after `feedback_picker`. Differences:
//! - The option list comes from `theme::build_picker_list()` which includes
//!   both built-in presets and custom `.toml` files from
//!   `~/.config/deepseek/themes/`.
//! - Up/Down emit a `ConfigUpdated{persist:false}` so the host swaps
//!   `app.theme` immediately and the whole TUI re-paints under the
//!   modal — the user sees the candidate theme before committing.
//! - Enter persists (`persist:true`); Esc emits one more
//!   `ConfigUpdated{persist:false}` to restore the original theme name
//!   that was active when the picker opened.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::tui::theme::ThemePickerEntry;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

pub struct ThemePickerView {
    /// Full list of pickable entries (built-in + custom).
    entries: Vec<ThemePickerEntry>,
    selected: usize,
    /// Settings name of the theme that was active when the picker opened.
    original_name: String,
}

impl ThemePickerView {
    #[must_use]
    pub fn new(original_name: String) -> Self {
        let entries = crate::tui::theme::build_picker_list();

        // Match the current theme name (which may be the internal `Theme.name`
        // like "whale" or a `file:midnight` string, or the canonical "dark").
        let normalized = original_name.trim().to_ascii_lowercase();
        let selected = entries
            .iter()
            .position(|e| {
                e.setting_name() == normalized || e.to_theme().name == normalized.as_str()
            })
            .or_else(|| {
                // Fallback: if the original is a bare name like "midnight",
                // try `file:midnight`.
                let as_file = format!("file:{normalized}");
                entries.iter().position(|e| e.setting_name() == as_file)
            })
            .unwrap_or(0);

        Self {
            entries,
            selected,
            original_name,
        }
    }

    fn current(&self) -> &ThemePickerEntry {
        self.entries
            .get(self.selected)
            .unwrap_or(&ThemePickerEntry::Builtin(
                crate::tui::theme::ThemeId::System,
            ))
    }

    fn preview_event(&self) -> ViewAction {
        ViewAction::Emit(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().setting_name(),
            persist: false,
        })
    }

    fn commit_event(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().setting_name(),
            persist: true,
        })
    }

    fn revert_event(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.original_name.clone(),
            persist: false,
        })
    }

    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    fn move_down(&mut self) {
        let max = self.entries.len().saturating_sub(1);
        if self.selected < max {
            self.selected += 1;
        }
    }
}

impl ModalView for ThemePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ThemePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => self.revert_event(),
            KeyCode::Enter => self.commit_event(),
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                self.preview_event()
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                self.preview_event()
            }
            KeyCode::Home => {
                self.selected = 0;
                self.preview_event()
            }
            KeyCode::End => {
                self.selected = self.entries.len().saturating_sub(1);
                self.preview_event()
            }
            KeyCode::Char(c)
                if matches!(c, '1'..='9')
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let idx = (c as usize) - ('1' as usize);
                if idx < self.entries.len() {
                    self.selected = idx;
                    self.preview_event()
                } else {
                    ViewAction::None
                }
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 78u16.min(area.width.saturating_sub(4));
        let needed_height = (self.entries.len() as u16).saturating_add(9);
        let popup_height = needed_height.min(area.height.saturating_sub(4));

        if popup_width == 0 || popup_height == 0 {
            return;
        }

        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(popup_width)) / 2,
            y: area.y + (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        let live = self.current().to_theme();

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Theme ",
                Style::default()
                    .fg(live.status_working)
                    .add_modifier(Modifier::BOLD),
            )))
            .title_bottom(Line::from(vec![
                Span::styled(" ↑/↓ ", Style::default().fg(live.text_muted)),
                Span::raw("preview "),
                Span::styled(" Enter ", Style::default().fg(live.text_muted)),
                Span::raw("save "),
                Span::styled(" Esc ", Style::default().fg(live.text_muted)),
                Span::raw("cancel"),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(live.border_color))
            .style(Style::default().bg(live.surface_bg))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        // Render rows
        let max_visible = inner.height as usize;
        let start = if self.entries.len() <= max_visible {
            0
        } else {
            self.selected
                .saturating_sub(max_visible.saturating_sub(1).min(2))
                .min(self.entries.len().saturating_sub(max_visible))
        };

        let end = (start + max_visible).min(self.entries.len());
        let visible = &self.entries[start..end];

        for (i, entry) in visible.iter().enumerate() {
            let global = start + i;
            let row_y = inner.y + i as u16;

            let is_selected = global == self.selected;
            let row_bg = if is_selected {
                live.selection_bg
            } else {
                live.surface_bg
            };
            let row_fg = if is_selected {
                live.text_body
            } else {
                live.text_muted
            };

            let label = entry.display_label();
            let tagline = entry.tagline();
            let row_theme = entry.to_theme();

            // Swatch bar: 5×2 chars coloured with theme accent colors
            let swatch = vec![
                Span::styled("  ", Style::default().bg(row_theme.surface_bg)),
                Span::styled("  ", Style::default().bg(row_theme.panel_bg)),
                Span::styled("  ", Style::default().bg(row_theme.status_working)),
                Span::styled("  ", Style::default().bg(row_theme.mode_yolo)),
                Span::styled("  ", Style::default().bg(row_theme.mode_plan)),
            ];

            let pointer = if is_selected { "▶" } else { " " };
            let number_style = Style::default().fg(live.text_hint);
            let row_style = if is_selected {
                Style::default()
                    .fg(live.text_body)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(row_fg)
            };

            let mut spans: Vec<Span> = Vec::with_capacity(8);
            spans.push(Span::styled(format!(" {pointer} "), row_style));
            spans.push(Span::styled(format!("{}. ", global + 1), number_style));
            spans.push(Span::styled(format!("{:<22}", label), row_style));
            spans.extend(swatch);
            spans.push(Span::raw("  "));
            spans.push(Span::styled(tagline, Style::default().fg(live.text_muted)));

            let row = Line::from(spans);
            // Clear the entire row to avoid leftovers from narrower previous frames
            for x in inner.x..inner.x + inner.width {
                if let Some(cell) = buf.cell_mut((x, row_y)) {
                    cell.set_style(Style::default().bg(row_bg));
                }
            }
            Paragraph::new(row)
                .style(Style::default().bg(row_bg))
                .render(
                    Rect {
                        x: inner.x,
                        y: row_y,
                        width: inner.width,
                        height: 1,
                    },
                    buf,
                );
        }
    }
}
