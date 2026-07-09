//! `/theme` picker with live preview.
//!
//! Modeled after `feedback_picker`. Differences:
//! - The option list comes from `palette::SELECTABLE_THEMES`.
//! - Up/Down emit a `ConfigUpdated{persist:false}` so the host swaps
//!   `app.ui_theme` immediately and the whole TUI re-paints under the
//!   modal — the user sees the candidate theme before committing.
//! - Enter persists (`persist:true`); Esc emits one more
//!   `ConfigUpdated{persist:false}` to restore the original theme name
//!   that was active when the picker opened.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};

use crate::palette::{SELECTABLE_THEMES, ThemeId, UiTheme};
use crate::tui::views::{
    ActionHint, ModalKind, ModalView, ViewAction, ViewEvent, centered_modal_area,
    render_modal_footer,
};

pub struct ThemePickerView {
    selected: usize,
    /// Settings name of the theme that was active when the picker opened.
    /// Used to revert on Esc.
    original_name: String,
    /// Cached UiTheme for `ThemeId::System`, captured once at construction
    /// so the per-frame render doesn't re-invoke `UiTheme::detect()` (which
    /// reads `COLORFGBG`) on every keystroke.
    system_ui_theme: UiTheme,
}

impl ThemePickerView {
    #[must_use]
    pub fn new(original_name: String) -> Self {
        // If the persisted name matches one of the entries, start there;
        // otherwise fall back to "System" so the cursor lands on a valid row.
        let selected = SELECTABLE_THEMES
            .iter()
            .position(|id| id.name() == original_name.trim().to_ascii_lowercase())
            .unwrap_or(0);
        Self {
            selected,
            original_name,
            system_ui_theme: UiTheme::detect(),
        }
    }

    fn current(&self) -> ThemeId {
        SELECTABLE_THEMES
            .get(self.selected)
            .copied()
            .unwrap_or(ThemeId::System)
    }

    /// Resolve a theme to a `UiTheme`, returning the cached `System`
    /// resolution to avoid repeated env-var reads inside `render`.
    fn ui_theme_for(&self, id: ThemeId) -> UiTheme {
        if matches!(id, ThemeId::System) {
            self.system_ui_theme
        } else {
            id.ui_theme()
        }
    }

    fn preview_event(&self) -> ViewAction {
        ViewAction::Emit(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().name().to_string(),
            persist: false,
        })
    }

    fn commit_event(&self) -> ViewAction {
        ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
            key: "theme".to_string(),
            value: self.current().name().to_string(),
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
        self.selected = (self.selected + SELECTABLE_THEMES.len() - 1) % SELECTABLE_THEMES.len();
    }

    fn move_down(&mut self) {
        self.selected = (self.selected + 1) % SELECTABLE_THEMES.len();
    }
}

impl ModalView for ThemePickerView {
    fn kind(&self) -> ModalKind {
        ModalKind::ThemePicker
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_up(),
            MouseEventKind::ScrollDown => self.move_down(),
            _ => {}
        }
        ViewAction::None
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
                self.selected = SELECTABLE_THEMES.len().saturating_sub(1);
                self.preview_event()
            }
            // Number shortcuts: '1'..='9' jump to that row (1-indexed).
            // '0' is rejected explicitly — saturating_sub would otherwise
            // collapse it onto row 0, which is unintuitive.
            KeyCode::Char(c)
                if matches!(c, '1'..='9')
                    && !key.modifiers.contains(KeyModifiers::CONTROL)
                    && !key.modifiers.contains(KeyModifiers::ALT) =>
            {
                let idx = (c as usize) - ('1' as usize);
                if idx < SELECTABLE_THEMES.len() {
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
        // 1 title + 1 spacer + N rows + spacer + the in-body action footer.
        // centered_modal_area clamps strictly to `area`, so the modal always
        // fits even on tiny or split-pane terminals.
        let needed_height = (SELECTABLE_THEMES.len() as u16).saturating_add(9);
        let popup_area = centered_modal_area(area, 78, needed_height, 44, 8);

        // The live theme has already been swapped under us via ConfigUpdated,
        // so we pull the *current* preview's UiTheme from the cursor row to
        // skin the modal chrome. That way the popup itself shifts color as
        // the cursor moves, matching what the background will look like
        // after Enter. We keep the live `surface_bg` (not the shared ink) and
        // the bare `Clear` so the preview backdrop reads as intended.
        let live = self.ui_theme_for(self.current());

        Clear.render(popup_area, buf);

        let block = Block::default()
            .title(Line::from(Span::styled(
                " Theme ",
                Style::default()
                    .fg(live.status_working)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(live.border))
            .style(Style::default().bg(live.surface_bg))
            .padding(Padding::uniform(1));

        let inner = block.inner(popup_area);
        block.render(popup_area, buf);

        let content = render_modal_footer(
            inner,
            buf,
            &[
                ActionHint::new("↑/↓", "preview"),
                ActionHint::new("Enter", "save"),
                ActionHint::new("Esc", "revert"),
            ],
        );

        let mut lines: Vec<Line> = Vec::with_capacity(SELECTABLE_THEMES.len() + 5);
        lines.push(Line::from(Span::styled(
            "Pick a theme — preview is live; Enter saves to settings.toml.",
            Style::default().fg(live.text_muted),
        )));
        lines.push(Line::from(""));

        for (idx, id) in SELECTABLE_THEMES.iter().enumerate() {
            let id = *id;
            let is_selected = idx == self.selected;
            let row_style = if is_selected {
                Style::default()
                    .fg(live.text_body)
                    .bg(live.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(live.text_body)
            };
            let tagline_style = if is_selected {
                Style::default().fg(live.text_muted).bg(live.selection_bg)
            } else {
                Style::default().fg(live.text_dim)
            };
            let number_style = if is_selected {
                Style::default()
                    .fg(live.status_working)
                    .bg(live.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(live.text_hint)
            };
            let pointer = if is_selected { "▶" } else { " " };

            // 3-cell color swatch per row using the candidate theme's own
            // accent + panel + border colors so the picker doubles as a
            // legend. Use the cached resolver so `System` doesn't repeat
            // `UiTheme::detect()`.
            let row_theme = self.ui_theme_for(id);
            let swatch = vec![
                Span::styled("  ", Style::default().bg(row_theme.surface_bg)),
                Span::styled("  ", Style::default().bg(row_theme.panel_bg)),
                Span::styled("  ", Style::default().bg(row_theme.status_working)),
                Span::styled("  ", Style::default().bg(row_theme.mode_yolo)),
                Span::styled("  ", Style::default().bg(row_theme.mode_plan)),
            ];

            let mut spans: Vec<Span> = Vec::with_capacity(8);
            spans.push(Span::styled(format!(" {pointer} "), row_style));
            spans.push(Span::styled(format!("{}. ", idx + 1), number_style));
            spans.push(Span::styled(
                format!("{:<22}", id.display_name()),
                row_style,
            ));
            spans.extend(swatch);
            spans.push(Span::raw("  "));

            let prefix_width = Line::from(spans.clone()).width();
            let tagline = crate::tui::ui_text::semantic_truncate(
                id.tagline(),
                usize::from(content.width).saturating_sub(prefix_width),
            );
            spans.push(Span::styled(tagline, tagline_style));

            lines.push(Line::from(spans));
        }

        Paragraph::new(lines).render(content, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn selected_name(action: &ViewAction) -> Option<&str> {
        match action {
            ViewAction::Emit(ViewEvent::ConfigUpdated { key, value, .. })
            | ViewAction::EmitAndClose(ViewEvent::ConfigUpdated { key, value, .. })
                if key == "theme" =>
            {
                Some(value.as_str())
            }
            _ => None,
        }
    }

    #[test]
    fn opens_at_persisted_theme() {
        let v = ThemePickerView::new("tokyo-night".to_string());
        assert_eq!(v.current(), ThemeId::TokyoNight);
    }

    #[test]
    fn unknown_persisted_name_falls_back_to_first_row() {
        let v = ThemePickerView::new("not-a-real-theme".to_string());
        assert_eq!(v.selected, 0);
        assert_eq!(v.current(), ThemeId::System);
    }

    #[test]
    fn arrow_down_previews_next_theme() {
        let mut v = ThemePickerView::new("system".to_string());
        let action = v.handle_key(key(KeyCode::Down));
        assert!(matches!(action, ViewAction::Emit(_)));
        assert_eq!(selected_name(&action), Some(ThemeId::Terminal.name()));
    }

    #[test]
    fn arrow_navigation_wraps_at_picker_edges() {
        let mut v = ThemePickerView::new("system".to_string());
        let last = SELECTABLE_THEMES.last().unwrap();

        let action = v.handle_key(key(KeyCode::Up));
        assert_eq!(selected_name(&action), Some(last.name()));

        let action = v.handle_key(key(KeyCode::Down));
        assert_eq!(selected_name(&action), Some(SELECTABLE_THEMES[0].name()));
    }

    #[test]
    fn enter_commits_with_persist_true() {
        let mut v = ThemePickerView::new("system".to_string());
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down));
        v.handle_key(key(KeyCode::Down)); // -> CatppuccinMocha
        let action = v.handle_key(key(KeyCode::Enter));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "theme");
                assert_eq!(value, ThemeId::CatppuccinMocha.name());
                assert!(persist);
            }
            other => panic!("expected commit, got {other:?}"),
        }
    }

    #[test]
    fn esc_reverts_to_original() {
        let mut v = ThemePickerView::new("dracula".to_string());
        v.handle_key(key(KeyCode::Up));
        v.handle_key(key(KeyCode::Up));
        let action = v.handle_key(key(KeyCode::Esc));
        match action {
            ViewAction::EmitAndClose(ViewEvent::ConfigUpdated {
                key,
                value,
                persist,
            }) => {
                assert_eq!(key, "theme");
                assert_eq!(value, "dracula");
                assert!(!persist);
            }
            other => panic!("expected revert, got {other:?}"),
        }
    }

    #[test]
    fn digit_jumps_to_row() {
        let mut v = ThemePickerView::new("system".to_string());
        let action = v.handle_key(key(KeyCode::Char('6')));
        // Row 6 (1-indexed) -> index 5 -> CatppuccinMocha
        assert_eq!(
            selected_name(&action),
            Some(ThemeId::CatppuccinMocha.name())
        );
    }

    #[test]
    fn digit_zero_is_rejected_not_remapped_to_row_zero() {
        let mut v = ThemePickerView::new("dracula".to_string());
        let before = v.selected;
        let action = v.handle_key(key(KeyCode::Char('0')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(v.selected, before, "'0' should not move the cursor");
    }

    #[test]
    fn render_does_not_panic_on_zero_sized_area() {
        // The picker historically panicked here via .max(W).max(H) floors
        // that produced dimensions larger than the available area, then
        // underflowed the centering arithmetic.
        let v = ThemePickerView::new("system".to_string());
        let outer = ratatui::layout::Rect::new(0, 0, 10, 10);
        let area = ratatui::layout::Rect::new(0, 0, 0, 0);
        let mut buf = ratatui::buffer::Buffer::empty(outer);
        v.render(area, &mut buf);
    }

    #[test]
    fn render_does_not_panic_on_tiny_area() {
        // 20×6 is smaller than every soft floor the picker prefers.
        let v = ThemePickerView::new("system".to_string());
        let area = ratatui::layout::Rect::new(0, 0, 20, 6);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        v.render(area, &mut buf);
    }

    #[test]
    fn render_semantically_truncates_taglines_at_narrow_width() {
        let v = ThemePickerView::new("system".to_string());
        let area = ratatui::layout::Rect::new(0, 0, 56, 12);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        v.render(area, &mut buf);
        let rows = (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let text = rows.join("\n");

        assert!(text.contains('…'), "{text}");
        for (idx, row) in rows.iter().enumerate() {
            assert!(
                crate::tui::ui_text::text_display_width(row) <= usize::from(area.width),
                "line {idx} overflows: {row:?}"
            );
        }
    }

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn theme_picker_is_usable_and_opaque_at_blocker_sizes() {
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
            stack.push(ThemePickerView::new("system".to_string()));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in ["preview", "save", "revert"] {
                assert!(text.contains(label), "{w}x{h}: missing footer '{label}'");
            }
            assert!(
                !text.contains('X'),
                "{w}x{h}: background bleed-through into modal surface"
            );
            // The theme picker paints the *live* theme surface (not the shared
            // ink), so assert the center cell is painted (no surviving
            // sentinel) rather than checking a fixed background color.
            assert_ne!(
                buf[(w / 2, h / 2)].symbol(),
                "X",
                "{w}x{h}: modal interior must be painted"
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
