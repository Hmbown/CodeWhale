//! Searchable help overlay for `?`, `F1`, and `Ctrl+/`.
//!
//! Renders two stacked sections — *Slash commands* and *Keybindings* — with
//! a live substring filter applied as the user types in the search box. The
//! command list is sourced from [`crate::commands::command_infos()`] and the
//! keybinding list from [`crate::tui::keybindings::KEYBINDINGS`] so neither
//! can drift from the wired-up handlers.
//!
//! Keys: any printable character extends the filter, `Backspace` (or `Ctrl+H`)
//! shrinks it,
//! `↑`/`↓` (or `Ctrl+P`/`Ctrl+N`) move the selection, `PgUp`/`PgDn` jump by
//! ten rows, `Home`/`End` jump to ends, and `Esc` closes. Pressing `?` again
//! at the call-site (`tui::ui`) also toggles the overlay closed.

use std::borrow::Cow;
use std::cell::RefCell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::commands;
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::hit_region::HitMap;
use crate::tui::keybindings::KEYBINDINGS;
use crate::tui::views::{ActionHint, ModalKind, ModalView, ViewAction, render_modal_chrome};

/// Top-level sections rendered in the overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpSection {
    Concept,
    Command,
    Keybinding,
}

impl HelpSection {
    fn label(self, locale: Locale) -> Cow<'static, str> {
        match self {
            Self::Concept => Cow::Borrowed("Concepts"),
            Self::Command => tr(locale, MessageId::HelpSlashCommands),
            Self::Keybinding => tr(locale, MessageId::HelpKeybindings),
        }
    }

    /// Sort key: concepts first, then command list, then keybindings.
    fn rank(self) -> u8 {
        match self {
            Self::Concept => 0,
            Self::Command => 1,
            Self::Keybinding => 2,
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

struct HelpConcept {
    label: &'static str,
    description: &'static str,
    search_terms: &'static str,
}

const HELP_CONCEPTS: &[HelpConcept] = &[
    HelpConcept {
        label: "Fleet / workers",
        description: "Operate-mode roster of model workers; open or stop rows from the control surface.",
        search_terms: "fleet worker workers operate roster models providers subagents open stop",
    },
    HelpConcept {
        label: "Config / scope",
        description: "Settings, workspace scope, context panel, sidebar, theme, Hotbar, and setup surfaces.",
        search_terms: "config settings scope workspace context sidebar theme hotbar setup",
    },
    HelpConcept {
        label: "Models / providers / routing",
        description: "Model picker, provider routes, automatic routing, effort, fallback, and budget facts.",
        search_terms: "model provider routing route auto effort fallback budget tokens",
    },
    HelpConcept {
        label: "Permissions / trust",
        description: "Act, Plan, and Operate stay separate from approval/trust and full-access permissions.",
        search_terms: "permissions trust approval act plan operate full access yolo bypass",
    },
    HelpConcept {
        label: "Tool calls / receipts",
        description: "Transcript tool rows, raw detail with v, turn inspector, receipts, and audit trails.",
        search_terms: "tool calls receipts transcript raw detail v inspector audit",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelpRenderRow {
    Section(HelpSection),
    Entry { slot: usize, entry_idx: usize },
}

pub struct HelpView {
    locale: Locale,
    entries: Vec<HelpEntry>,
    /// Indices into `entries`, in display order, after filtering.
    filtered: Vec<usize>,
    query: String,
    selected: usize,
    /// Row hit regions from the last paint (shared by hover + click).
    hit_map: RefCell<HitMap>,
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
            hit_map: RefCell::new(HitMap::new()),
        };
        view.refilter();
        view
    }

    fn tr(&self, id: MessageId) -> Cow<'static, str> {
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

    fn move_selection_wrapping(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.filtered.len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.selected = next;
    }

    fn render_rows(&self) -> Vec<HelpRenderRow> {
        let mut rows = Vec::new();
        let mut active_section: Option<HelpSection> = None;

        for (slot, entry_idx) in self.filtered.iter().copied().enumerate() {
            let entry = &self.entries[entry_idx];
            if active_section != Some(entry.section) {
                rows.push(HelpRenderRow::Section(entry.section));
                active_section = Some(entry.section);
            }
            rows.push(HelpRenderRow::Entry { slot, entry_idx });
        }

        rows
    }

    fn selected_render_row(rows: &[HelpRenderRow], selected: usize) -> usize {
        rows.iter()
            .position(|row| matches!(row, HelpRenderRow::Entry { slot, .. } if *slot == selected))
            .unwrap_or(0)
    }

    fn visible_row_start(rows: &[HelpRenderRow], selected: usize, visible_budget: usize) -> usize {
        if rows.len() <= visible_budget {
            return 0;
        }

        let selected_row = Self::selected_render_row(rows, selected);
        let half = visible_budget / 2;
        if selected_row <= half {
            0
        } else if selected_row + half >= rows.len() {
            rows.len().saturating_sub(visible_budget)
        } else {
            selected_row.saturating_sub(half)
        }
    }

    fn focus_row_at(&mut self, column: u16, row: u16) -> bool {
        let Some(slot) = self.hit_map.borrow().row_at(column, row) else {
            return false;
        };
        if slot >= self.filtered.len() {
            return false;
        }
        self.selected = slot;
        true
    }
}

fn build_entries(locale: Locale) -> Vec<HelpEntry> {
    let mut entries = Vec::new();

    for (idx, concept) in HELP_CONCEPTS.iter().enumerate() {
        let label = concept.label.to_string();
        let description = concept.description.to_string();
        let haystack = format!(
            "{} {} {}",
            label.to_ascii_lowercase(),
            description.to_ascii_lowercase(),
            concept.search_terms.to_ascii_lowercase()
        );
        entries.push(HelpEntry {
            section: HelpSection::Concept,
            sub_rank: idx as u8,
            label,
            description,
            haystack,
        });
    }

    for command in commands::command_infos() {
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        // Scroll clamps at the ends (keyboard Up/Down wrap); wheel-wrapping
        // reads as disorienting.
        match mouse.kind {
            MouseEventKind::ScrollUp => self.move_selection(-1),
            MouseEventKind::ScrollDown => self.move_selection(1),
            MouseEventKind::Moved
            | MouseEventKind::Drag(MouseButton::Left)
            | MouseEventKind::Down(MouseButton::Left) => {
                let _ = self.focus_row_at(mouse.column, mouse.row);
            }
            _ => {}
        }
        ViewAction::None
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        match key.code {
            KeyCode::Esc => ViewAction::Close,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                ViewAction::Close
            }
            KeyCode::Char('q') | KeyCode::Char('Q') if self.query.is_empty() => ViewAction::Close,
            KeyCode::Up => {
                self.move_selection_wrapping(-1);
                ViewAction::None
            }
            KeyCode::Down => {
                self.move_selection_wrapping(1);
                ViewAction::None
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection_wrapping(-1);
                ViewAction::None
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.move_selection_wrapping(1);
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
        // Shared modal chrome recipe (COH-05): centered opaque surface, titled
        // border, and action-hint footer in one call.
        let chrome = render_modal_chrome(
            area,
            buf,
            Line::from(vec![Span::styled(
                format!(
                    " {} — {} ",
                    self.tr(MessageId::HelpTitle),
                    self.tr(MessageId::HelpSubtitle)
                ),
                Style::default()
                    .fg(palette::WHALE_ACCENT_PRIMARY)
                    .add_modifier(Modifier::BOLD),
            )]),
            90,
            28,
            44,
            8,
            // Empty hint keys keep the existing localized footer phrases as
            // plain labels (the help footer uses prose, not key/label pairs).
            &[
                ActionHint::new("", self.tr(MessageId::HelpFooterTypeFilter)),
                ActionHint::new("", self.tr(MessageId::HelpFooterMove)),
                ActionHint::new("", self.tr(MessageId::HelpFooterJump)),
                ActionHint::new("", self.tr(MessageId::HelpFooterClose)),
            ],
        );
        let content = chrome.body;

        let mut lines: Vec<Line<'static>> = Vec::new();
        let mut hit_map = HitMap::new();

        let query_label = if self.query.is_empty() {
            self.tr(MessageId::HelpFilterPlaceholder).to_string()
        } else {
            format!("{}{}", self.tr(MessageId::HelpFilterPrefix), self.query)
        };
        lines.push(Line::from(Span::styled(
            query_label,
            Style::default()
                .fg(palette::WHALE_INFO)
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
            let inner_width = content.width as usize;
            let label_width = 28.min(inner_width.saturating_sub(8));
            let desc_capacity = inner_width.saturating_sub(label_width + 4);

            // `content` is the body area above the wrapping footer (the block's
            // border, padding, and footer rows already removed), so budgeting
            // against its height keeps selected rows clear of the footer.
            let header_lines = lines.len();
            let visible_budget = (content.height as usize)
                .saturating_sub(header_lines)
                .max(1);

            let rows = self.render_rows();
            let row_start = Self::visible_row_start(&rows, self.selected, visible_budget);

            for row in rows.iter().skip(row_start).take(visible_budget) {
                match *row {
                    HelpRenderRow::Section(section) => {
                        let count = self
                            .filtered
                            .iter()
                            .filter(|idx| self.entries[**idx].section == section)
                            .count();
                        lines.push(Line::from(Span::styled(
                            format!("  {} ({})", section.label(self.locale), count),
                            Style::default()
                                .fg(palette::WHALE_ACCENT_PRIMARY)
                                .add_modifier(Modifier::BOLD),
                        )));
                    }
                    HelpRenderRow::Entry { slot, entry_idx } => {
                        let entry = &self.entries[entry_idx];
                        let line_y = content.y.saturating_add(lines.len() as u16);
                        hit_map.push_row(slot, content.x, line_y, content.width);
                        let is_selected = slot == self.selected;
                        let style = if is_selected {
                            Style::default()
                                .fg(palette::SELECTION_TEXT)
                                .bg(palette::SELECTION_BG)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(palette::TEXT_PRIMARY)
                        };
                        let cursor = if is_selected { "▶ " } else { "  " };
                        let label = truncate_to_width(&entry.label, label_width);
                        let desc = truncate_to_width(&entry.description, desc_capacity);
                        let line_text = format!("{cursor}{label:<label_width$}  {desc}",);
                        lines.push(Line::from(Span::styled(line_text, style)));
                    }
                }
            }
        }

        *self.hit_map.borrow_mut() = hit_map;
        Paragraph::new(lines).render(content, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::hit_region::HitTarget;
    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };

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
        // Total = concept pages + registered slash commands + catalogued keybindings.
        let expected = HELP_CONCEPTS.len() + commands::command_infos().len() + KEYBINDINGS.len();
        assert_eq!(view.filtered.len(), expected);
        assert_eq!(view.entries.len(), expected);
    }

    #[test]
    fn concept_reference_entries_cover_required_topics() {
        let view = HelpView::new();
        let labels: Vec<&str> = view
            .entries
            .iter()
            .filter(|entry| entry.section == HelpSection::Concept)
            .map(|entry| entry.label.as_str())
            .collect();

        assert_eq!(
            labels,
            vec![
                "Fleet / workers",
                "Config / scope",
                "Models / providers / routing",
                "Permissions / trust",
                "Tool calls / receipts",
            ]
        );
    }

    #[test]
    fn search_crosses_concepts_commands_and_keybindings() {
        let mut concepts = HelpView::new();
        type_filter(&mut concepts, "fleet workers");
        assert!(
            concepts.filtered.iter().any(|idx| {
                let entry = &concepts.entries[*idx];
                entry.section == HelpSection::Concept && entry.label == "Fleet / workers"
            }),
            "fleet worker concept should be searchable"
        );

        let mut commands = HelpView::new();
        type_filter(&mut commands, "mode [act");
        assert!(commands.filtered.iter().any(|idx| {
            let entry = &commands.entries[*idx];
            entry.section == HelpSection::Command && entry.label == "/mode"
        }));

        let mut keybindings = HelpView::new();
        type_filter(&mut keybindings, "ctrl+r");
        assert!(keybindings.filtered.iter().any(|idx| {
            let entry = &keybindings.entries[*idx];
            entry.section == HelpSection::Keybinding && entry.label.eq_ignore_ascii_case("ctrl+r")
        }));
    }

    #[test]
    fn substring_filter_narrows_to_command() {
        let mut view = HelpView::new();
        type_filter(&mut view, "mode [act");
        assert!(!view.filtered.is_empty());
        // Every filtered entry should genuinely contain the query in its
        // searchable haystack — no false positives slipped past.
        for idx in &view.filtered {
            assert!(
                view.entries[*idx].haystack.contains("mode [act"),
                "entry {:?} leaked through `mode [act` filter",
                view.entries[*idx]
            );
        }
        // The unified `/mode` command must surface when filtering for a
        // concrete mode value from the visible vocabulary.
        assert!(
            view.filtered
                .iter()
                .any(|idx| view.entries[*idx].label == "/mode"),
            "/mode should match the `mode [act` filter"
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
        // Near-miss against the still-visible mode vocabulary so the last
        // character removes a unique miss and broadens the match set.
        type_filter(&mut view, "modez");
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
        type_filter(&mut view, "modez");
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
    fn ctrl_c_closes_overlay() {
        let mut view = HelpView::new();
        let action = view.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn q_closes_empty_filter_but_types_when_filtering() {
        let mut view = HelpView::new();
        let action = view.handle_key(key(KeyCode::Char('q')));
        assert!(matches!(action, ViewAction::Close));

        let mut view = HelpView::new();
        type_filter(&mut view, "mod");
        let action = view.handle_key(key(KeyCode::Char('q')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(view.query, "modq");
    }

    #[test]
    fn arrow_keys_move_selection_and_wrap_edges() {
        let mut view = HelpView::new();
        // Down once → row 1; Up twice wraps from the first row to the last.
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.selected, 1);
        view.handle_key(key(KeyCode::Up));
        view.handle_key(key(KeyCode::Up));
        assert_eq!(view.selected, view.filtered.len() - 1);
        // Down from last wraps to first; End still jumps to the last row.
        view.handle_key(key(KeyCode::Down));
        assert_eq!(view.selected, 0);
        view.handle_key(key(KeyCode::End));
        assert_eq!(view.selected, view.filtered.len() - 1);
    }

    #[test]
    fn visible_window_keeps_selected_entry_visible_after_scroll() {
        let mut view = HelpView::new();
        let selected = view
            .filtered
            .iter()
            .position(|idx| view.entries[*idx].label == "/home")
            .expect("/home command should be present");
        view.selected = selected;

        let rows = view.render_rows();
        let row_start = HelpView::visible_row_start(&rows, view.selected, 12);
        let visible = &rows[row_start..(row_start + 12).min(rows.len())];

        assert!(
            visible
                .iter()
                .any(|row| matches!(row, HelpRenderRow::Entry { slot, .. } if *slot == selected)),
            "selected help entry should stay in the visible render window"
        );
    }

    #[test]
    fn render_keeps_next_row_after_help_visible() {
        let mut view = HelpView::new();
        let help_slot = view
            .filtered
            .iter()
            .position(|idx| view.entries[*idx].label == "/help")
            .expect("/help command should be present");
        view.selected = help_slot;
        view.handle_key(key(KeyCode::Down));
        let selected_idx = view.filtered[view.selected];
        let selected_label = view.entries[selected_idx].label.clone();

        let area = Rect::new(0, 0, 96, 32);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let mut highlighted_label = false;
        for y in area.top()..area.bottom() {
            let mut row = String::new();
            let mut row_has_highlight = false;
            for x in area.left()..area.right() {
                let cell = &buf[(x, y)];
                row.push_str(cell.symbol());
                row_has_highlight |=
                    cell.bg == palette::SELECTION_BG && cell.fg == palette::SELECTION_TEXT;
            }
            if row_has_highlight && row.contains(&selected_label) {
                highlighted_label = true;
                break;
            }
        }

        assert!(
            highlighted_label,
            "selected row after /help should stay visibly highlighted"
        );
    }

    #[test]
    fn selected_help_row_uses_selection_highlight() {
        let view = HelpView::new();
        let area = Rect::new(0, 0, 96, 32);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let mut found_highlight = false;
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                let cell = &buf[(x, y)];
                if cell.bg == palette::SELECTION_BG && cell.fg == palette::SELECTION_TEXT {
                    found_highlight = true;
                    break;
                }
            }
        }

        assert!(
            found_highlight,
            "selected row should use the semantic selection highlight"
        );
    }

    #[test]
    fn mouse_hover_and_click_share_keyboard_selected_row() {
        let mut view = HelpView::new();
        let area = Rect::new(0, 0, 96, 32);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let hit = view
            .hit_map
            .borrow()
            .regions_for_test()
            .iter()
            .find(|region| matches!(region.target, HitTarget::Row { index } if index > 0))
            .copied()
            .expect("visible non-first help row hit region");
        let HitTarget::Row { index } = hit.target;

        let hover = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(hover, ViewAction::None));
        assert_eq!(view.selected, index);

        view.selected = 0;
        let click = view.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: hit.rect.x.saturating_add(2),
            row: hit.rect.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(matches!(click, ViewAction::None));
        assert_eq!(view.selected, index);
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
        assert!(
            dump.contains("Concepts"),
            "missing concepts section heading:\n{dump}"
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
        type_filter(&mut view, "mode [act");
        let area = Rect::new(0, 0, 96, 24);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);

        let dump = buffer_text(&buf, area);
        assert!(
            dump.contains("Filter: mode [act"),
            "filter echo missing:\n{dump}"
        );
        assert!(
            dump.contains("matches"),
            "match counter missing in dump:\n{dump}"
        );
        assert!(
            dump.contains("/mode"),
            "expected /mode command in filtered render:\n{dump}"
        );
        assert!(
            !dump.contains("/model"),
            "non-matching commands should not render under a `mode [act` filter:\n{dump}"
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

    /// The four terminal sizes the v0.8.66 modal blocker (#3732) requires
    /// every overlay to remain readable and fully operable at.
    const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

    #[test]
    fn help_is_usable_and_opaque_at_blocker_sizes() {
        use crate::tui::views::ViewStack;
        for (w, h) in BLOCKER_SIZES {
            let area = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(area);
            for y in 0..h {
                for x in 0..w {
                    buf[(x, y)].set_symbol("X");
                }
            }
            let mut stack = ViewStack::new();
            stack.push(HelpView::new_for_locale(Locale::En));
            stack.render(area, &mut buf);

            let rows: Vec<String> = (0..h)
                .map(|y| {
                    (0..w)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect();
            let text = rows.join("\n");

            for label in [
                "type to filter",
                "Up/Down move",
                "PgUp/PgDn jump",
                "Esc close",
            ] {
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
