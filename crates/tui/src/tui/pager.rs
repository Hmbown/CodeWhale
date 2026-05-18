//! Full-screen pager overlay for long outputs.
//!
//! Vim-style key bindings (mirroring the codex pager_overlay):
//! - `j` / Down — scroll down one line
//! - `k` / Up — scroll up one line
//! - `g g` / Home — jump to top
//! - `G` / End — jump to bottom
//! - `Ctrl+D` — half-page down
//! - `Ctrl+U` — half-page up
//! - `Ctrl+F` / PageDown / Space — full page down
//! - `Ctrl+B` / PageUp / Shift+Space — full page up
//! - `/` — start search; `n` / `N` — next / previous match
//! - `c` / `y` — copy the entire pager body to the system clipboard
//! - `q` / Esc — close pager

use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

/// Footer hint shown along the bottom border of the pager. Kept short so it
/// fits on narrow terminals; full reference lives in the module docs.
const FOOTER_HINT_NAV: &str =
    " j/k scroll  Space page  Ctrl+D/U half  g/G top/bottom  / search  c copy";
const FOOTER_HINT_EXIT: &str = " q/Esc close ";

pub struct PagerView {
    title: String,
    lines: Vec<Line<'static>>,
    plain_lines: Vec<String>,
    scroll: usize,
    search_input: String,
    search_matches: Vec<usize>,
    search_index: usize,
    search_mode: bool,
    pending_g: bool,
    /// Cached visible content height from the last render. Used by paging
    /// keys (Ctrl+D/U, Ctrl+F/B, Space, etc.) to compute scroll deltas
    /// without access to the render area.
    last_visible_height: Cell<usize>,
}

impl PagerView {
    pub fn new(title: impl Into<String>, lines: Vec<Line<'static>>) -> Self {
        let plain_lines = lines.iter().map(line_to_string).collect();
        Self {
            title: title.into(),
            lines,
            plain_lines,
            scroll: 0,
            search_input: String::new(),
            search_matches: Vec::new(),
            search_index: 0,
            search_mode: false,
            pending_g: false,
            last_visible_height: Cell::new(0),
        }
    }

    pub fn from_text(title: impl Into<String>, text: &str, width: u16) -> Self {
        let mut lines = Vec::new();
        for raw in text.lines() {
            for wrapped in wrap_text(raw, width.max(1) as usize) {
                lines.push(Line::from(Span::raw(wrapped)));
            }
            if raw.is_empty() {
                lines.push(Line::from(""));
            }
        }
        Self::new(title, lines)
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: usize, max_scroll: usize) {
        self.scroll = (self.scroll + amount).min(max_scroll);
    }

    fn scroll_to_top(&mut self) {
        self.scroll = 0;
    }

    fn scroll_to_bottom(&mut self, max_scroll: usize) {
        self.scroll = max_scroll;
    }

    /// Plain-text body of the pager joined with `\n`, suitable for sending
    /// to the system clipboard via `ViewEvent::CopyToClipboard`. Reflects the
    /// content the user sees, including any width-based wrapping that
    /// `from_text` introduced — copying the visible text is the expected
    /// affordance when the user can't reach terminal-native selection inside
    /// the modal (#1354).
    pub fn body_text(&self) -> String {
        self.plain_lines.join("\n")
    }

    /// Return the page height (in lines) used for paging keys.
    ///
    /// Falls back to a small constant (10) before the first render so the
    /// pager still responds to paging keys when invoked synthetically (e.g.
    /// in unit tests). After the first render, the cached value reflects
    /// the actual visible content area.
    fn page_height(&self) -> usize {
        let cached = self.last_visible_height.get();
        if cached == 0 { 10 } else { cached }
    }

    /// Half a page, rounded up so a single press always moves at least one line.
    fn half_page_height(&self) -> usize {
        let page = self.page_height();
        page.div_ceil(2).max(1)
    }

    fn max_scroll(&self) -> usize {
        // Match the render-side clamp so G/End land at the visible bottom and
        // k/Up immediately scroll back up by one line.
        self.lines.len().saturating_sub(self.page_height())
    }

    fn start_search(&mut self) {
        self.search_mode = true;
        self.search_input.clear();
        self.search_matches.clear();
        self.search_index = 0;
    }

    fn update_search_matches(&mut self) {
        let query = self.search_input.trim();
        if query.is_empty() {
            self.search_matches.clear();
            self.search_index = 0;
            return;
        }
        let lower = query.to_ascii_lowercase();
        self.search_matches = self
            .plain_lines
            .iter()
            .enumerate()
            .filter_map(|(idx, line)| {
                if line.to_ascii_lowercase().contains(&lower) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        self.search_index = 0;
    }

    fn jump_to_match(&mut self) {
        if let Some(&line) = self.search_matches.get(self.search_index) {
            self.scroll = line;
        }
    }

    fn next_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_index = (self.search_index + 1) % self.search_matches.len();
        self.jump_to_match();
    }

    fn prev_match(&mut self) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_index == 0 {
            self.search_index = self.search_matches.len().saturating_sub(1);
        } else {
            self.search_index = self.search_index.saturating_sub(1);
        }
        self.jump_to_match();
    }
}

impl ModalView for PagerView {
    fn kind(&self) -> ModalKind {
        ModalKind::Pager
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if self.search_mode {
            match key.code {
                KeyCode::Enter => {
                    self.search_mode = false;
                    self.update_search_matches();
                    self.jump_to_match();
                    return ViewAction::None;
                }
                KeyCode::Esc => {
                    // Bail out of search mode AND drop the current match list
                    // so the user gets back to the un-highlighted view —
                    // codex-style behavior. To resume from where they left
                    // off they re-enter `/` and re-type.
                    self.search_mode = false;
                    self.search_input.clear();
                    self.search_matches.clear();
                    self.search_index = 0;
                    return ViewAction::None;
                }
                KeyCode::Backspace => {
                    self.search_input.pop();
                    return ViewAction::None;
                }
                KeyCode::Char(c) => {
                    self.search_input.push(c);
                    return ViewAction::None;
                }
                _ => {}
            }
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let max_scroll = self.max_scroll();

        // Ctrl+chord paging keys are matched first because their KeyCode
        // also matches the bare `KeyCode::Char(c)` arms below.
        if ctrl {
            match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.scroll_down(self.half_page_height(), max_scroll);
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.scroll_up(self.half_page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    self.scroll_down(self.page_height(), max_scroll);
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.scroll_up(self.page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_up(1);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_down(1, max_scroll);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.scroll_up(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.scroll_down(self.page_height(), max_scroll);
                self.pending_g = false;
                ViewAction::None
            }
            // Vim convention: Space pages down, Shift+Space pages up. Match
            // Shift+Space first so it is not absorbed by the bare ' ' arm.
            KeyCode::Char(' ') if shift => {
                self.scroll_up(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char(' ') => {
                self.scroll_down(self.page_height(), max_scroll);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Home => {
                self.scroll_to_top();
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::End => {
                self.scroll_to_bottom(max_scroll);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('g') => {
                if self.pending_g {
                    self.scroll_to_top();
                    self.pending_g = false;
                } else {
                    self.pending_g = true;
                }
                ViewAction::None
            }
            KeyCode::Char('G') => {
                self.scroll_to_bottom(max_scroll);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('/') => {
                self.start_search();
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('n') => {
                self.next_match();
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('N') => {
                self.prev_match();
                self.pending_g = false;
                ViewAction::None
            }
            // Copy the entire pager body to the clipboard. The pager
            // intercepts mouse capture so terminal-native selection is
            // disabled inside it; without this binding users with no
            // out-of-band copy path would have no way to extract content
            // they can see (#1354). Both `c` and `y` are wired so users
            // landing from either OS-clipboard or vim convention find a
            // working key.
            KeyCode::Char('c') | KeyCode::Char('y') => {
                self.pending_g = false;
                ViewAction::Emit(ViewEvent::CopyToClipboard {
                    text: self.body_text(),
                    label: "Pager content".to_string(),
                })
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.scroll_up(3);
                self.pending_g = false;
                ViewAction::None
            }
            MouseEventKind::ScrollDown => {
                self.scroll_down(3, self.max_scroll());
                self.pending_g = false;
                ViewAction::None
            }
            _ => ViewAction::None,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let popup_width = area.width.saturating_sub(2).max(1);
        let popup_height = area.height.saturating_sub(2).max(1);
        let popup_area = Rect {
            x: 1,
            y: 1,
            width: popup_width,
            height: popup_height,
        };

        Clear.render(popup_area, buf);

        // Borders eat 1 row top + 1 row bottom; the block's `Padding::uniform(1)`
        // eats 1 more on each side. Net: 4 rows of overhead to subtract from
        // `popup_area.height` before we know how many lines fit.
        let mut visible_height = popup_area.height.saturating_sub(4) as usize;
        if self.search_mode {
            // Reserve a row for the search prompt that gets pushed below.
            visible_height = visible_height.saturating_sub(1);
        } else if !self.search_matches.is_empty() {
            // Reserve a row for the "match X/Y (n/N)" status; without this
            // the status line gets clipped on small popup heights and the
            // user can't see how many matches there are.
            visible_height = visible_height.saturating_sub(1);
        }
        // Cache for paging keys; the value is treated as advisory and
        // clamped at use-time.
        self.last_visible_height.set(visible_height);
        let max_scroll = self.lines.len().saturating_sub(visible_height);
        let scroll = self.scroll.min(max_scroll);
        let end = (scroll + visible_height).min(self.lines.len());
        let mut visible_lines = if self.lines.is_empty() {
            vec![Line::from("")]
        } else {
            self.lines[scroll..end].to_vec()
        };

        // Highlight matched lines while the search prompt is closed and the
        // user is navigating with `n` / `N`. Other matches get a subtle
        // background; the current match gets a louder one. Per-substring
        // highlighting is deferred to a follow-up — preserving the pre-styled
        // spans (assistant / system colors) through a substring re-style is
        // a separate concern.
        if !self.search_mode && !self.search_matches.is_empty() {
            let current_match_line = self.search_matches.get(self.search_index).copied();
            for (visible_idx, line) in visible_lines.iter_mut().enumerate() {
                let absolute_idx = scroll + visible_idx;
                if absolute_idx >= self.lines.len() {
                    break;
                }
                if !self.search_matches.contains(&absolute_idx) {
                    continue;
                }
                let is_current = current_match_line == Some(absolute_idx);
                let bg = if is_current {
                    Color::Yellow
                } else {
                    Color::DarkGray
                };
                let fg = if is_current {
                    Color::Reset
                } else {
                    Color::Yellow
                };
                let highlight = Style::default().bg(bg).fg(fg).add_modifier(Modifier::BOLD);
                for span in line.spans.iter_mut() {
                    span.style = highlight;
                }
            }
        }

        if self.search_mode {
            let prompt = format!("/{}", self.search_input);
            visible_lines.push(Line::from(Span::styled(
                prompt,
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            )));
        } else if !self.search_matches.is_empty() {
            let status = format!(
                "match {}/{} (n/N)",
                self.search_index + 1,
                self.search_matches.len()
            );
            visible_lines.push(Line::from(Span::styled(
                status,
                Style::default().fg(palette::TEXT_MUTED),
            )));
        }

        let footer = Line::from(vec![
            Span::styled(
                FOOTER_HINT_EXIT,
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(FOOTER_HINT_NAV, Style::default().fg(palette::TEXT_HINT)),
        ]);
        let block = Block::default()
            .title(self.title.clone())
            .title_bottom(footer)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .padding(Padding::uniform(1));

        let paragraph = Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false });
        paragraph.render(popup_area, buf);
    }
}

fn line_to_string(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.to_string())
        .collect::<String>()
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = word.width();
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            push_word_breaking_chars(word, width, &mut current, &mut current_width, &mut lines);
            continue;
        }
        let additional = if current.is_empty() {
            word_width
        } else {
            word_width + 1
        };
        if current_width + additional > width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

fn push_word_breaking_chars(
    word: &str,
    width: usize,
    current: &mut String,
    current_width: &mut usize,
    lines: &mut Vec<String>,
) {
    for ch in word.chars() {
        let char_width = ch.width().unwrap_or(1);
        if *current_width + char_width > width && *current_width > 0 {
            lines.push(std::mem::take(current));
            *current_width = 0;
        }
        current.push(ch);
        *current_width += char_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;

    fn make_pager(lines: usize) -> PagerView {
        let lines: Vec<Line<'static>> = (0..lines)
            .map(|i| Line::from(format!("line-{i:03}")))
            .collect();
        PagerView::new("T", lines)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn key_mod(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// Drive a render once so `last_visible_height` is populated and paging
    /// keys use a deterministic page size.
    fn prime_layout(view: &mut PagerView, height: u16) {
        let area = Rect::new(0, 0, 40, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
    }

    #[test]
    fn j_scrolls_down_one_line() {
        let mut p = make_pager(50);
        let _ = p.handle_key(key(KeyCode::Char('j')));
        assert_eq!(p.scroll, 1);
    }

    #[test]
    fn k_scrolls_up_one_line() {
        let mut p = make_pager(50);
        p.scroll = 5;
        let _ = p.handle_key(key(KeyCode::Char('k')));
        assert_eq!(p.scroll, 4);
    }

    #[test]
    fn gg_jumps_to_top() {
        let mut p = make_pager(50);
        p.scroll = 30;
        let _ = p.handle_key(key(KeyCode::Char('g')));
        assert!(p.pending_g, "first 'g' should arm pending_g");
        assert_eq!(p.scroll, 30, "first 'g' alone must not scroll");
        let _ = p.handle_key(key(KeyCode::Char('g')));
        assert_eq!(p.scroll, 0);
        assert!(!p.pending_g);
    }

    #[test]
    fn home_jumps_to_top() {
        let mut p = make_pager(50);
        p.scroll = 30;
        let _ = p.handle_key(key(KeyCode::Home));
        assert_eq!(p.scroll, 0);
    }

    #[test]
    fn shift_g_jumps_to_bottom() {
        let mut p = make_pager(50);
        let _ = p.handle_key(key(KeyCode::Char('G')));
        assert_eq!(p.scroll, p.max_scroll());
    }

    #[test]
    fn end_jumps_to_bottom() {
        let mut p = make_pager(50);
        let _ = p.handle_key(key(KeyCode::End));
        assert_eq!(p.scroll, p.max_scroll());
    }

    #[test]
    fn up_immediately_scrolls_after_shift_g_to_bottom() {
        let mut p = make_pager(50);
        prime_layout(&mut p, 22);
        let bottom = p.max_scroll();

        let _ = p.handle_key(key(KeyCode::Char('G')));
        assert_eq!(p.scroll, bottom);
        let _ = p.handle_key(key(KeyCode::Up));
        assert_eq!(p.scroll, bottom - 1);
        let _ = p.handle_key(key(KeyCode::Char('k')));
        assert_eq!(p.scroll, bottom - 2);
    }

    #[test]
    fn k_immediately_scrolls_after_end_to_bottom() {
        let mut p = make_pager(50);
        prime_layout(&mut p, 22);
        let bottom = p.max_scroll();

        let _ = p.handle_key(key(KeyCode::End));
        assert_eq!(p.scroll, bottom);
        let _ = p.handle_key(key(KeyCode::Char('k')));
        assert_eq!(p.scroll, bottom - 1);
    }

    #[test]
    fn ctrl_d_half_page_down() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        let half = p.half_page_height();
        assert!(half >= 1, "half-page must move at least one line");
        let _ = p.handle_key(ctrl(KeyCode::Char('d')));
        assert_eq!(p.scroll, half);
    }

    #[test]
    fn ctrl_u_half_page_up() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        p.scroll = 50;
        let half = p.half_page_height();
        let _ = p.handle_key(ctrl(KeyCode::Char('u')));
        assert_eq!(p.scroll, 50 - half);
    }

    #[test]
    fn ctrl_f_full_page_down() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        let page = p.page_height();
        let _ = p.handle_key(ctrl(KeyCode::Char('f')));
        assert_eq!(p.scroll, page);
    }

    #[test]
    fn ctrl_b_full_page_up() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        p.scroll = 80;
        let page = p.page_height();
        let _ = p.handle_key(ctrl(KeyCode::Char('b')));
        assert_eq!(p.scroll, 80 - page);
    }

    #[test]
    fn space_pages_down() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        let page = p.page_height();
        let _ = p.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(p.scroll, page);
    }

    #[test]
    fn shift_space_pages_up() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        p.scroll = 80;
        let page = p.page_height();
        let _ = p.handle_key(key_mod(KeyCode::Char(' '), KeyModifiers::SHIFT));
        assert_eq!(p.scroll, 80 - page);
    }

    #[test]
    fn page_down_uses_cached_visible_height() {
        let mut p = make_pager(200);
        prime_layout(&mut p, 22);
        let page = p.page_height();
        let _ = p.handle_key(key(KeyCode::PageDown));
        assert_eq!(p.scroll, page);
    }

    #[test]
    fn q_closes_pager() {
        let mut p = make_pager(10);
        let action = p.handle_key(key(KeyCode::Char('q')));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn esc_closes_pager() {
        let mut p = make_pager(10);
        let action = p.handle_key(key(KeyCode::Esc));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn g_does_not_consume_search_input() {
        // While in search mode, 'g' must be treated as a search character,
        // not as the half of a `gg` jump-to-top sequence.
        let mut p = make_pager(50);
        p.scroll = 10;
        let _ = p.handle_key(key(KeyCode::Char('/')));
        assert!(p.search_mode);
        let _ = p.handle_key(key(KeyCode::Char('g')));
        assert_eq!(p.search_input, "g");
        assert_eq!(p.scroll, 10);
    }

    #[test]
    fn footer_hint_includes_new_bindings() {
        // The rendered pager must surface the new vim-style bindings to
        // the user; check the footer hint covers the headline keys.
        for needle in &[
            "j/k",
            "g/G",
            "Space",
            "Ctrl+D",
            "/ search",
            "c copy",
            "q/Esc close",
        ] {
            let full_hint = format!("{FOOTER_HINT_EXIT}{FOOTER_HINT_NAV}");
            assert!(
                full_hint.contains(needle),
                "footer hint missing {needle:?}: {full_hint}"
            );
        }
    }

    #[test]
    fn c_emits_copy_event_with_full_body() {
        // #1354: the pager intercepts mouse capture, so users have no way to
        // copy content out without an in-app key. Both `c` and `y` should
        // emit a CopyToClipboard event carrying the whole body so the host
        // dispatcher (in ui.rs) can write through `app.clipboard` and toast
        // a confirmation.
        let mut p = make_pager(3);
        let action = p.handle_key(key(KeyCode::Char('c')));
        match action {
            ViewAction::Emit(ViewEvent::CopyToClipboard { text, label }) => {
                assert_eq!(text, "line-000\nline-001\nline-002");
                assert_eq!(label, "Pager content");
            }
            other => panic!("expected CopyToClipboard emit, got {other:?}"),
        }
    }

    #[test]
    fn y_emits_copy_event_for_vim_users() {
        let mut p = make_pager(3);
        let action = p.handle_key(key(KeyCode::Char('y')));
        assert!(
            matches!(action, ViewAction::Emit(ViewEvent::CopyToClipboard { .. })),
            "y must emit a copy event for vim-yank parity"
        );
    }

    #[test]
    fn copy_keys_inert_in_search_mode() {
        // Within `/`-search mode `c` and `y` must be treated as search
        // characters, not as a copy trigger — otherwise users typing a
        // query that contains either letter would lose their input.
        let mut p = make_pager(10);
        let _ = p.handle_key(key(KeyCode::Char('/')));
        assert!(p.search_mode);
        let action = p.handle_key(key(KeyCode::Char('c')));
        assert!(matches!(action, ViewAction::None));
        assert_eq!(p.search_input, "c");
    }

    #[test]
    fn footer_hint_is_rendered_in_buffer() {
        let p = make_pager(5);
        let area = Rect::new(0, 0, 100, 10);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);
        // The pager renders into an inset popup_area = (1, 1, w-2, h-2),
        // so the bottom border lives at y = popup_area.bottom() - 1, not
        // at the outer area's last row.
        let popup_bottom_y = (area.height as usize).saturating_sub(2);
        let mut bottom = String::new();
        for x in 1..area.right().saturating_sub(1) {
            bottom.push_str(buf[(x, popup_bottom_y as u16)].symbol());
        }
        assert!(
            bottom.contains("close") || bottom.contains("scroll"),
            "expected footer hint on bottom border row {popup_bottom_y}, got: {bottom:?}"
        );
    }

    /// `/` opens the search prompt; typing chars accumulates them; Enter
    /// commits and jumps to the first match. The matches index/count line
    /// must surface in the rendered buffer afterwards.
    #[test]
    fn search_finds_matches_and_renders_match_counter() {
        let mut p = make_pager(20);
        prime_layout(&mut p, 16);

        // Open search.
        let _ = p.handle_key(key(KeyCode::Char('/')));
        // Type "5" to match line-005, line-015 (any line whose number contains
        // a 5 — make_pager produced "line-NNN" with three-digit indices).
        for ch in "5".chars() {
            let _ = p.handle_key(key(KeyCode::Char(ch)));
        }
        // Commit.
        let _ = p.handle_key(key(KeyCode::Enter));

        // Render and look for the "match X/Y" status line.
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);
        let mut full = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                full.push_str(buf[(x, y)].symbol());
            }
            full.push('\n');
        }
        assert!(
            full.contains("match 1/2") || full.contains("match 1/3"),
            "expected match counter; got buffer:\n{full}"
        );
    }

    /// Esc while in search mode bails out AND clears the highlighted matches
    /// so the un-highlighted view returns. (Codex parity.)
    #[test]
    fn esc_in_search_mode_clears_matches() {
        let mut p = make_pager(20);
        prime_layout(&mut p, 16);

        let _ = p.handle_key(key(KeyCode::Char('/')));
        let _ = p.handle_key(key(KeyCode::Char('5')));
        let _ = p.handle_key(key(KeyCode::Enter));
        assert!(!p.search_matches.is_empty());

        // Re-enter search mode and Esc out — matches must clear.
        let _ = p.handle_key(key(KeyCode::Char('/')));
        let _ = p.handle_key(key(KeyCode::Esc));
        assert!(p.search_matches.is_empty());
        assert_eq!(p.search_input, "");
        assert!(!p.search_mode);
    }

    /// `n` and `N` cycle forward and backward through matches, wrapping at
    /// the ends without panicking on out-of-bounds index.
    #[test]
    fn n_and_capital_n_cycle_matches_with_wrap() {
        let mut p = make_pager(50);
        prime_layout(&mut p, 16);

        // Search "1" — matches every line whose printed index contains a 1.
        let _ = p.handle_key(key(KeyCode::Char('/')));
        let _ = p.handle_key(key(KeyCode::Char('1')));
        let _ = p.handle_key(key(KeyCode::Enter));
        let total = p.search_matches.len();
        assert!(total > 1, "test needs multiple matches, got {total}");

        let start = p.search_index;
        let _ = p.handle_key(key(KeyCode::Char('n')));
        assert_eq!(p.search_index, (start + 1) % total);
        let _ = p.handle_key(key(KeyCode::Char('N')));
        assert_eq!(p.search_index, start);

        // Wrap backwards from 0 → last.
        let _ = p.handle_key(key(KeyCode::Char('N')));
        assert_eq!(p.search_index, total - 1);
        let _ = p.handle_key(key(KeyCode::Char('n')));
        assert_eq!(p.search_index, 0);
    }

    /// While search matches exist and the prompt is closed, the matched
    /// lines are visually distinguished in the rendered buffer by their
    /// background color. We sample directly across the matched-line text
    /// columns rather than the whole row width because Paragraph leaves
    /// the trailing-area cells at the default style.
    #[test]
    fn matched_lines_get_highlight_background() {
        let mut p = make_pager(20);
        prime_layout(&mut p, 16);

        let _ = p.handle_key(key(KeyCode::Char('/')));
        let _ = p.handle_key(key(KeyCode::Char('5')));
        let _ = p.handle_key(key(KeyCode::Enter));
        assert!(!p.search_matches.is_empty());

        let area = Rect::new(0, 0, 40, 16);
        let mut buf = Buffer::empty(area);
        p.render(area, &mut buf);

        // Text starts at popup_area.x + block_border_left + padding_left
        // = 1 + 1 + 1 = 3. The fixture text is "line-NNN" (8 chars) so we
        // sample 3..11. The current-match row is the top of the visible
        // window because `jump_to_match` set scroll = match_line.
        let popup_top_y = 1 /* outer popup */ + 1 /* block top border */ + 1 /* padding top */;
        let mut found_highlight = false;
        for x in 3..11 {
            let bg = buf[(x, popup_top_y)].style().bg;
            if matches!(bg, Some(Color::Yellow) | Some(Color::DarkGray)) {
                found_highlight = true;
                break;
            }
        }
        assert!(
            found_highlight,
            "expected a Yellow/DarkGray highlight cell on the matched-line text columns"
        );
    }

    #[test]
    fn mouse_scroll_up_scrolls_content() {
        let mut p = make_pager(50);
        p.scroll = 10;
        let action = p.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(p.scroll, 7);
        assert!(matches!(action, ViewAction::None));
    }

    #[test]
    fn mouse_scroll_down_scrolls_content() {
        let mut p = make_pager(50);
        prime_layout(&mut p, 20);
        p.scroll = 10;
        let action = p.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(p.scroll, 13);
        assert!(matches!(action, ViewAction::None));
    }

    #[test]
    fn mouse_scroll_down_clamps_to_pager_bottom() {
        let mut p = make_pager(50);
        prime_layout(&mut p, 20);
        let bottom = p.max_scroll();

        for _ in 0..100 {
            let _ = p.handle_mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            });
        }

        assert_eq!(p.scroll, bottom);
    }

    #[test]
    fn wrap_text_breaks_overlong_cjk_runs() {
        let text = "这是一个非常长的中文字符串".repeat(10);
        let lines = wrap_text(&text, 16);

        for line in &lines {
            assert!(line.width() <= 16, "line {line:?} exceeds width 16");
        }

        assert_eq!(lines.join(""), text);
    }
}
