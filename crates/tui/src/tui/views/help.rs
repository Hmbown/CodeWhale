//! Searchable help overlay for `?`, `F1`, and `Ctrl+/`.
//!
//! Renders two stacked sections — *Slash commands* and *Keybindings* — with
//! a live substring filter applied as the user types in the search box. The
//! command list is sourced from [`crate::commands::COMMANDS`] and the
//! keybinding list from [`crate::tui::keybindings::KEYBINDINGS`] so neither
//! can drift from the wired-up handlers.
//!
//! Keys: any printable character extends the filter, `Backspace` (or `Ctrl+H`)
//! shrinks it,
//! `↑`/`↓` (or `Ctrl+P`/`Ctrl+N`) move the selection, `PgUp`/`PgDn` jump by
//! ten rows, `Home`/`End` jump to ends, and `Esc` closes. Pressing `?` again
//! at the call-site (`tui::ui`) also toggles the overlay closed.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::commands;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::keybindings::KEYBINDINGS;
use crate::tui::views::{ModalKind, ModalView, ViewAction};

/// Two top-level sections rendered in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpSection {
    Command,
    Keybinding,
}

impl HelpSection {
    fn label(self, locale: Locale) -> &'static str {
        match self {
            Self::Command => tr(locale, MessageId::HelpSlashCommands),
            Self::Keybinding => tr(locale, MessageId::HelpKeybindings),
        }
    }

    /// Sort key — commands before keybindings keeps the most-used surface up
    /// top so an unfiltered overlay opens with the user's likely target in
    /// view without scrolling.
    fn rank(self) -> u8 {
        match self {
            Self::Command => 0,
            Self::Keybinding => 1,
        }
    }
}

#[derive(Debug, Clone)]
struct HelpEntry {
    section: HelpSection,
    /// Sort-within-section key — keybinding entries reuse their declared
    /// section's rank so the help overlay groups Navigation, Editing, … in
    /// the same order as `tui::keybindings`.
    sub_rank: u8,
    label: String,
    description: String,
    /// Lowercased haystack used for substring matching; pre-built so each
    /// keystroke does not re-allocate per entry.
    haystack: String,
}

pub struct HelpView {
    locale: Locale,
    entries: Vec<HelpEntry>,
    /// Indices into `entries`, in display order, after filtering.
    filtered: Vec<usize>,
    query: String,
    selected: usize,
}

impl Default for HelpView {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpView {
    pub fn new() -> Self {
        Self::new_for_locale(Locale::En)
    }

    pub fn new_for_locale(locale: Locale) -> Self {
        let entries = build_entries(locale);
        let mut view = Self {
            locale,
            entries,
            filtered: Vec::new(),
            query: String::new(),
            selected: 0,
        };
        view.refilter();
        view
    }

    fn tr(&self, id: MessageId) -> &'static str {
        tr(self.locale, id)
    }

    fn refilter(&mut self) {
        // Substring matching is intentional — fuzzy matchers can hide the
        // exact-prefix hit a user is typing toward, which is the wrong
        // failure mode for a *help* surface. We split on whitespace so
        // multi-term queries (`apply mode`) act as an AND.
        let query = self.query.trim().to_ascii_lowercase();
        let terms: Vec<&str> = query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect();

        let mut filtered: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| terms.iter().all(|term| entry.haystack.contains(term)))
            .map(|(idx, _)| idx)
            .collect();

        filtered.sort_by_key(|idx| {
            let entry = &self.entries[*idx];
            (entry.section.rank(), entry.sub_rank, entry.label.clone())
        });
        self.filtered = filtered;
        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).clamp(0, len - 1) as usize;
        self.selected = next;
    }
}

fn build_entries(locale: Locale) -> Vec<HelpEntry> {
    let mut entries = Vec::new();

    for command in commands::COMMANDS {
        let label = format!("/{}", command.name);
        let localized = command.description_for(locale);
        let description = if command.aliases.is_empty() {
            localized.to_string()
        } else {
            format!(
                "{}  (aliases: {})",
                localized,
                command
                    .aliases
                    .iter()
                    .map(|a| format!("/{a}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let haystack = format!(
            "{} {} {}",
            label.to_ascii_lowercase(),
            description.to_ascii_lowercase(),
            command.usage.to_ascii_lowercase()
        );
        entries.push(HelpEntry {
            section: HelpSection::Command,
            // Commands have no inherent ordering — fall back to alphabetical
            // by leaning on `label.clone()` in the final sort_by_key tuple.
            sub_rank: 0,
            label,
            description,
            haystack,
        });
    }

    for binding in KEYBINDINGS {
        let label = binding.chord.to_string();
        let description = format!(
            "[{}] {}",
            binding.section.label(locale),
            tr(locale, binding.description_id)
        );
        let haystack = format!(
            "{} {}",
            label.to_ascii_lowercase(),
            description.to_ascii_lowercase()
        );
        entries.push(HelpEntry {
            section: HelpSection::Keybinding,
            sub_rank: binding.section.rank(),
            label,
            description,
            haystack,
        });
    }

    entries
}

fn modal_block() -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::BORDER_COLOR))
        .style(Style::default().bg(palette::DEEPSEEK_INK))
        .padding(Padding::uniform(1))
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text.width() <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    let limit = max_width.saturating_sub(1);
    for ch in text.chars() {
        let next_width = out.width() + ch.to_string().width();
        if next_width > limit {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

impl ModalView for HelpView {
    fn kind(&self) -> ModalKind {
        ModalKind::Help
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Up => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.move_selection(-10);
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.move_selection(10);
                ViewAction::None
            }
            KeyCode::Home => {
                self.selected = 0;
                ViewAction::None
            }
            KeyCode::End => {
                if !self.filtered.is_empty() {
                    self.selected = self.filtered.len() - 1;
                }
                ViewAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                ViewAction::None
            }
            // Terminals where stty erase == ^H send Ctrl+H instead of
            // Backspace (DEL). Treat it identically so the filter input
            // works across all platforms (#958).
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.pop();
                self.refilter();
                ViewAction::None
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && (key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT) =>
            {
                self.query.push(c);
                self.refilter();
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = 90.min(area.width.saturating_sub(4));
        let popup_height = 28.min(area.height.saturating_sub(4));
        let popup_area = Rect {
            x: area.width.saturating_sub(popup_width) / 2,
            y: area.height.saturating_sub(popup_height) / 2,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        let mut lines: Vec<Line<'static>> = Vec::new();

        let query_label = if self.query.is_empty() {
            self.tr(MessageId::HelpFilterPlaceholder).to_string()
        } else {
            format!("{}{}", self.tr(MessageId::HelpFilterPrefix), self.query)
        };
        lines.push(Line::from(Span::styled(
            query_label,
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        )));

        let match_count = if self.query.is_empty() {
            format!("{} entries", self.entries.len())
        } else {
            format!("{} / {} matches", self.filtered.len(), self.entries.len())
        };
        lines.push(Line::from(Span::styled(
            match_count,
            Style::default()
                .fg(palette::TEXT_DIM)
                .add_modifier(Modifier::ITALIC),
        )));
        lines.push(Line::from(""));

        if self.filtered.is_empty() {
            lines.push(Line::from(Span::styled(
                self.tr(MessageId::HelpNoMatches),
                Style::default()
                    .fg(palette::TEXT_MUTED)
                    .add_modifier(Modifier::ITALIC),
            )));
        } else {
            // The chord/label column takes up to 28 cols on wide screens;
            // descriptions fill the remainder. Borders and padding eat 4
            // cells from each side (border 1 + padding 1) × 2.
            let inner_width = popup_width.saturating_sub(4) as usize;
            let label_width = 28.min(inner_width.saturating_sub(8));
            let desc_capacity = inner_width.saturating_sub(label_width + 4);

            // Visible window: header (3) + footer hint (handled by block);
            // budget the remaining rows for entries and inserted section
            // headings. Section headings can push us past the budget on tiny
            // terminals — we still render them because losing the heading is
            // worse than losing one trailing row of entries.
            let header_lines = lines.len();
            let visible_budget = (popup_height as usize)
                .saturating_sub(header_lines + 3)
                .max(1);

            // Centre the selected row in the visible window when it is far
            // down, otherwise keep the natural top-aligned listing.
            let scroll = self
                .selected
                .saturating_sub(visible_budget.saturating_sub(1));
            let mut active_section: Option<HelpSection> = None;
            let mut rendered_rows = 0usize;

            for (slot, idx) in self.filtered.iter().enumerate() {
                if slot < scroll {
                    continue;
                }
                if rendered_rows >= visible_budget {
                    break;
                }

                let entry = &self.entries[*idx];
                if active_section != Some(entry.section) {
                    if rendered_rows > 0 {
                        lines.push(Line::from(""));
                        rendered_rows += 1;
                    }
                    let count = self
                        .filtered
                        .iter()
                        .filter(|idx| self.entries[**idx].section == entry.section)
                        .count();
                    lines.push(Line::from(Span::styled(
                        format!("  {} ({})", entry.section.label(self.locale), count),
                        Style::default()
                            .fg(palette::DEEPSEEK_BLUE)
                            .add_modifier(Modifier::BOLD),
                    )));
                    rendered_rows += 1;
                    active_section = Some(entry.section);
                    if rendered_rows >= visible_budget {
                        break;
                    }
                }

                let is_selected = slot == self.selected;
                let style = if is_selected {
                    Style::default()
                        .fg(palette::SELECTION_TEXT)
                        .bg(palette::SELECTION_BG)
                } else {
                    Style::default().fg(palette::TEXT_PRIMARY)
                };
                let cursor = if is_selected { "▶ " } else { "  " };
                let label = truncate_to_width(&entry.label, label_width);
                let desc = truncate_to_width(&entry.description, desc_capacity);
                let line_text = format!("{cursor}{label:<label_width$}  {desc}", label = label,);
                lines.push(Line::from(Span::styled(line_text, style)));
                rendered_rows += 1;
            }
        }

        let block = modal_block()
            .title(Line::from(vec![Span::styled(
                format!(" {} ", self.tr(MessageId::HelpTitle)),
                Style::default()
                    .fg(palette::DEEPSEEK_BLUE)
                    .add_modifier(Modifier::BOLD),
            )]))
            .title_bottom(Line::from(vec![
                Span::styled(
                    self.tr(MessageId::HelpFooterTypeFilter),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    self.tr(MessageId::HelpFooterMove),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    self.tr(MessageId::HelpFooterJump),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
                Span::styled(
                    self.tr(MessageId::HelpFooterClose),
                    Style::default().fg(palette::TEXT_MUTED),
                ),
            ]));

        Paragraph::new(lines).block(block).render(popup_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_filter(view: &mut HelpView, text: &str) {
        for ch in text.chars() {
            view.handle_key(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        }
    }

    #[test]
    fn empty_filter_lists_all_entries() {
        let view = HelpView::new();
        // Total = registered slash commands + catalogued keybindings.
        let expected = commands::COMMANDS.len() + KEYBINDINGS.len();
        assert_eq!(view.filtered.len(), expected);
        assert_eq!(view.entries.len(), expected);
    }

    #[test]
    fn substring_filter_narrows_to_command() {
        let mut view = HelpView::new();
        type_filter(&mut view, "yolo");
        assert!(!view.filtered.is_empty());
        // Every filtered entry should genuinely contain the query in its
        // searchable haystack — no false positives slipped past.
        for idx in &view.filtered {
            assert!(
                view.entries[*idx].haystack.contains("yolo"),
                "entry {:?} leaked through `yolo` filter",
                view.entries[*idx]
            );
        }
        // The `/yolo` command must survive the filter; it's the canonical
        // single-term match.
        assert!(
            view.filtered
                .iter()
                .any(|idx| view.entries[*idx].label == "/yolo"),
            "/yolo should match the `yolo` filter"
        );
    }

    #[test]
    fn substring_filter_finds_keybinding_by_chord() {
        let mut view = HelpView::new();
        type_filter(&mut view, "ctrl+r");
        assert!(!view.filtered.is_empty(), "Ctrl+R should match");
        assert!(
            view.filtered
                .iter()
                .any(|idx| view.entries[*idx].label.eq_ignore_ascii_case("ctrl+r")),
            "Ctrl+R chord must surface in the filtered set"
        );
    }

    #[test]
    fn multiple_terms_act_as_and() {
        let mut view = HelpView::new();
        type_filter(&mut view, "session picker");
        assert!(
            !view.filtered.is_empty(),
            "expected at least one entry mentioning both `session` and `picker`"
        );
        for idx in &view.filtered {
            let haystack = &view.entries[*idx].haystack;
            assert!(
                haystack.contains("session") && haystack.contains("picker"),
                "entry {:?} leaked through `session picker` AND filter",
                view.entries[*idx]
            );
        }
    }

    #[test]
    fn unknown_filter_yields_empty_set() {
        let mut view = HelpView::new();
        type_filter(&mut view, "zzzqqxxnope");
        assert!(view.filtered.is_empty());
        assert_eq!(view.selected, 0);
    }

    #[test]
    fn backspace_widens_match_set() {
        let mut view = HelpView::new();
        type_filter(&mut view, "yolox");
        let narrow = view.filtered.len();
        view.handle_key(key(KeyCode::Backspace));
        let wider = view.filtered.len();
        assert!(
            wider > narrow,
            "backspace must broaden the matching set (was {narrow}, now {wider})"
        );
    }

    #[test]
    fn ctrl_h_widens_match_set() {
        let mut view = HelpView::new();
        type_filter(&mut view, "yolox");
        let narrow = view.filtered.len();
        view.handle_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        let wider = view.filtered.len();
        assert!(
            wider > narrow,
            "Ctrl+H must behave as Backspace, broadening the matching set (was {narrow}, now {wider})"
        );
    }

    #[test]
    fn esc_closes_overlay() {
        let mut view = HelpView::new();
        let action = view.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn arrow_keys_move_selection_within_bounds() {
        let mut view = HelpView::new();
        // Down once → row 1; Up twice → clamped at 0.
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.selected, 1);
        view.handle_key(key(KeyCode::Up));
        view.handle_key(key(KeyCode::Up));
        assert_eq!(view.selected, 0);
        // End → last row.
        view.handle_key(key(KeyCode::End));
        assert_eq!(view.selected, view.filtered.len() - 1);
    }

    #[test]
    fn render_includes_help_chrome_for_empty_filter() {
        let view = HelpView::new();
        let area = Rect::new(0, 0, 96, 32);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        // Title border + section headings should always render.
        assert!(dump.contains("Help"), "missing help title:\n{dump}");
        assert!(
            dump.contains("Type to filter"),
            "missing filter prompt:\n{dump}"
        );
        assert!(
            dump.contains("Slash commands"),
            "missing slash-command section heading:\n{dump}"
        );
        // Footer hint should advertise close key on the bottom border.
        assert!(
            dump.contains("Esc close"),
            "missing Esc close footer hint:\n{dump}"
        );
    }

    #[test]
    fn render_with_filter_shows_only_matching_section_and_status() {
        let mut view = HelpView::new();
        type_filter(&mut view, "yolo");
        let area = Rect::new(0, 0, 96, 24);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains("Filter: yolo"),
            "filter echo missing:\n{dump}"
        );
        assert!(
            dump.contains("matches"),
            "match counter missing in dump:\n{dump}"
        );
        assert!(
            dump.contains("/yolo"),
            "expected /yolo command in filtered render:\n{dump}"
        );
        assert!(
            !dump.contains("/agent"),
            "non-matching commands should not render under a `yolo` filter:\n{dump}"
        );
    }

    #[test]
    fn localized_help_chrome_renders_without_missing_markers() {
        let view = HelpView::new_for_locale(Locale::ZhHans);
        let area = Rect::new(0, 0, 48, 18);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains('帮') && dump.contains('助'),
            "missing localized title:\n{dump}"
        );
        assert!(
            !dump.contains("MISSING"),
            "missing-key marker leaked:\n{dump}"
        );
    }

    #[test]
    fn localized_help_keybinding_descriptions_use_zh_hans() {
        let entries = build_entries(Locale::ZhHans);
        let kb_entries: Vec<_> = entries
            .iter()
            .filter(|e| e.section == HelpSection::Keybinding)
            .collect();
        assert!(!kb_entries.is_empty(), "no keybinding entries found");

        for entry in &kb_entries {
            assert!(
                entry
                    .description
                    .chars()
                    .any(|c| { ('\u{4e00}'..='\u{9fff}').contains(&c) }),
                "keybinding description not localized: {}",
                entry.description
            );
        }
    }

    fn buffer_text(buf: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}
