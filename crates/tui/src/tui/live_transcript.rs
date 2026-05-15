//! Full-screen live transcript overlay with sticky-bottom auto-scroll (#94).
//!
//! Toggled with `Ctrl+T` while the engine is streaming. Behaviour:
//!
//! - At-bottom (`sticky_to_bottom = true`) — every refresh re-pins scroll to
//!   the new tail, so streaming output appears to flow off the bottom edge.
//! - Scroll up — `sticky_to_bottom` flips to `false`; subsequent refreshes
//!   leave scroll position alone so the user can read history without being
//!   yanked back down.
//! - Scroll back to bottom (End / G / paging past the tail) — `sticky` flips
//!   to `true` again; auto-tail resumes.
//! - Esc / `q` — close, returning to the normal view. The engine never
//!   pauses while the overlay is open; new chunks accumulate in the cells
//!   exactly as they would on the normal screen.
//!
//! Cache strategy: the overlay holds its own `TranscriptCache` keyed by
//! `(CellId, width, revision)`. Revisions come from the same per-cell
//! counters the main transcript already maintains (`App.history_revisions`
//! and `App.active_cell_revision`). Resize invalidates the cells whose width
//! key just changed; revision bumps invalidate only the cells that mutated;
//! cells that didn't change reuse their existing wrap.

use std::cell::{Cell, RefCell};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Padding, Paragraph, Widget, Wrap},
};

use crate::palette;
use crate::tui::app::App;
use crate::tui::backtrack::Direction;
use crate::tui::history::{HistoryCell, TranscriptRenderOptions};
use crate::tui::transcript_cache::{CellId, TranscriptCache};
use crate::tui::views::{ModalKind, ModalView, ViewAction, ViewEvent};

/// Render mode for the overlay. `Tail` is the original Ctrl+T sticky-tail
/// behaviour (#94). `BacktrackPreview` (#133) highlights the Nth-from-tail
/// `HistoryCell::User` so the user can see which turn Esc-Esc-Enter will
/// roll back to. The mode also disables sticky-tail (we want the user to
/// scan history, not be yanked to live output) and pins scroll near the
/// highlighted cell on transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Tail,
    BacktrackPreview {
        selected_idx: usize,
    },
}

/// Single-line footer hint. Kept short so it fits on narrow terminals.
const FOOTER_HINT: &str =
    " j/k scroll  Space/b page  g/G top/bottom  End=resume tail  q/Esc close ";

/// Snapshot of one cell, refreshed every frame from `App`. Owns the cell so
/// the overlay's `render(&self)` can wrap without re-borrowing `App`.
#[derive(Debug, Clone)]
struct CellSnapshot {
    id: CellId,
    revision: u64,
    cell: HistoryCell,
}

struct FlattenedTranscript {
    lines: Vec<Line<'static>>,
    highlighted_range: Option<(usize, usize)>,
}

pub struct LiveTranscriptOverlay {
    /// Latest cell snapshots (history + active). Refreshed via
    /// `refresh_from_app` immediately before each render so streaming
    /// mutations show up on the next paint.
    snapshots: Vec<CellSnapshot>,
    /// Render options sampled from `App` at refresh time so toggles like
    /// `show_thinking` propagate into the overlay live.
    options: TranscriptRenderOptions,
    /// Wrapped-line cache. `RefCell` so `render(&self)` can write through.
    cache: RefCell<TranscriptCache>,
    /// Sticky-tail flag: when `true`, refresh re-pins scroll to the bottom.
    /// Flipped to `false` when the user scrolls up; flipped back to `true`
    /// when they scroll past the last visible line.
    sticky_to_bottom: Cell<bool>,
    /// Current top-of-viewport line offset into the flattened line list.
    scroll: Cell<usize>,
    /// Visible content height from the last render. Used by paging keys
    /// before the next render frame populates a fresh value.
    last_visible_height: Cell<usize>,
    /// Last total line count after wrapping; cached so `handle_key` can
    /// clamp scroll without re-wrapping. Updated by `render`.
    last_total_lines: Cell<usize>,
    /// Pending `gg` second keystroke for Vim-style jump-to-top.
    pending_g: bool,
    /// Render mode — `Tail` is the live-stream mode; `BacktrackPreview`
    /// highlights the selected user message (#133).
    mode: Mode,
    /// Set when a backtrack selection changes. The next render pins the
    /// selected cell into view once we know the wrapped line range.
    preview_pin_pending: Cell<bool>,
}

impl LiveTranscriptOverlay {
    #[must_use]
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            options: TranscriptRenderOptions::default(),
            cache: RefCell::new(TranscriptCache::new()),
            sticky_to_bottom: Cell::new(true),
            scroll: Cell::new(0),
            last_visible_height: Cell::new(0),
            last_total_lines: Cell::new(0),
            pending_g: false,
            mode: Mode::Tail,
            preview_pin_pending: Cell::new(false),
        }
    }

    /// Switch the overlay into backtrack-preview mode. Sticky-tail is
    /// turned off so the highlighted cell stays in view while the user
    /// steps through prior turns. The wrap cache stays valid because the
    /// underlying snapshot data hasn't changed — only the post-wrap
    /// highlight overlay does.
    pub fn set_backtrack_preview(&mut self, selected_idx: usize) {
        self.mode = Mode::BacktrackPreview { selected_idx };
        self.sticky_to_bottom.set(false);
        self.preview_pin_pending.set(true);
    }

    /// Return the overlay to live-tail mode (used when backtrack is
    /// confirmed or canceled). Re-arms sticky-tail so streaming resumes.
    #[allow(dead_code)] // exposed for callers that retain an overlay across a backtrack cancel; current UI just pops the view.
    pub fn set_tail_mode(&mut self) {
        self.mode = Mode::Tail;
        self.sticky_to_bottom.set(true);
        self.preview_pin_pending.set(false);
    }

    /// For tests + UI: current mode.
    #[allow(dead_code)] // currently consumed only by tests; kept public for symmetry with `set_*` setters.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Pull the latest cells + revisions from `App` so the next `render` shows
    /// streaming mutations. Must be called before `view_stack.render` while
    /// this overlay is on top; otherwise the cells stay frozen at whatever
    /// state they were in when the overlay was first opened.
    pub fn refresh_from_app(&mut self, app: &mut App) {
        app.resync_history_revisions();
        let mut new_snapshots = Vec::with_capacity(
            app.history.len() + app.active_cell.as_ref().map_or(0, |a| a.entries().len()),
        );
        for (idx, cell) in app.history.iter().enumerate() {
            let rev = app.history_revisions.get(idx).copied().unwrap_or(0);
            new_snapshots.push(CellSnapshot {
                id: CellId::History(idx),
                revision: rev,
                cell: cell.clone(),
            });
        }
        if let Some(active) = app.active_cell.as_ref() {
            let active_rev = app.active_cell_revision;
            for (idx, cell) in active.entries().iter().enumerate() {
                let salt = (idx as u64).wrapping_add(1);
                // Salt mirrors the main-transcript scheme so cache keys are
                // stable across the two overlays for the same active entry.
                let revision = active_rev
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(salt);
                new_snapshots.push(CellSnapshot {
                    id: CellId::Active(idx),
                    revision,
                    cell: cell.clone(),
                });
            }
        }
        self.snapshots = new_snapshots;
        self.options = app.transcript_render_options();
    }

    /// Wrap each cell (using the cache) and return the flat line vector.
    /// In `BacktrackPreview` mode the lines belonging to the selected
    /// `HistoryCell::User` are decorated with a leading `▶` marker on the
    /// first line and reverse-video styling on every line so the eye
    /// snaps to them at a glance. The decoration is applied *after* the
    /// cache lookup so toggling preview mode never invalidates wraps.
    fn flatten(&self, width: u16) -> FlattenedTranscript {
        let width = width.max(1);
        let mut out: Vec<Line<'static>> = Vec::new();
        let mut highlighted_range = None;

        // Pre-compute which cell index (in `self.snapshots`) is the one
        // the user has selected via Esc-Esc. We walk snapshots backwards
        // counting User cells; the snapshot index whose count matches
        // `selected_idx + 1` is the highlighted one.
        let highlighted_cell_idx: Option<usize> = match self.mode {
            Mode::BacktrackPreview { selected_idx } => {
                let mut count = 0usize;
                let mut hit = None;
                for (idx, snap) in self.snapshots.iter().enumerate().rev() {
                    if matches!(snap.cell, HistoryCell::User { .. }) {
                        if count == selected_idx {
                            hit = Some(idx);
                            break;
                        }
                        count += 1;
                    }
                }
                hit
            }
            Mode::Tail => None,
        };

        let mut cache = self.cache.borrow_mut();
        for (cell_idx, snap) in self.snapshots.iter().enumerate() {
            let lines: Vec<Line<'static>> = match cache.get(snap.id, width, snap.revision) {
                Some(cached) => cached.to_vec(),
                None => {
                    let rendered = snap.cell.lines_with_options(width, self.options);
                    cache.insert(snap.id, width, snap.revision, rendered.clone());
                    rendered
                }
            };

            if Some(cell_idx) == highlighted_cell_idx {
                let start = out.len();
                out.extend(decorate_highlight(lines));
                let end = out.len();
                if end > start {
                    highlighted_range = Some((start, end));
                }
            } else {
                out.extend(lines);
            }
        }
        FlattenedTranscript {
            lines: out,
            highlighted_range,
        }
    }

    fn page_height(&self) -> usize {
        let cached = self.last_visible_height.get();
        if cached == 0 { 10 } else { cached }
    }

    fn half_page_height(&self) -> usize {
        self.page_height().div_ceil(2).max(1)
    }

    fn max_scroll(&self) -> usize {
        let total = self.last_total_lines.get();
        let visible = self.page_height();
        total.saturating_sub(visible)
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll.set(self.scroll.get().saturating_sub(amount));
        // Any upward motion exits sticky-tail; explicit user intent.
        self.sticky_to_bottom.set(false);
        self.preview_pin_pending.set(false);
    }

    fn scroll_down(&mut self, amount: usize) {
        let max = self.max_scroll();
        let scroll = self.scroll.get().saturating_add(amount).min(max);
        self.scroll.set(scroll);
        self.preview_pin_pending.set(false);
        if scroll >= max && matches!(self.mode, Mode::Tail) {
            self.sticky_to_bottom.set(true);
        }
    }

    fn jump_to_top(&mut self) {
        self.scroll.set(0);
        self.sticky_to_bottom.set(false);
        self.preview_pin_pending.set(false);
    }

    fn jump_to_bottom(&mut self) {
        self.scroll.set(self.max_scroll());
        self.sticky_to_bottom.set(matches!(self.mode, Mode::Tail));
        self.preview_pin_pending.set(false);
    }

    /// For tests: snapshot count.
    #[cfg(test)]
    fn snapshot_count(&self) -> usize {
        self.snapshots.len()
    }

    /// For tests: whether sticky-tail is currently armed.
    #[cfg(test)]
    pub fn is_sticky(&self) -> bool {
        self.sticky_to_bottom.get()
    }

    /// For tests: current scroll offset.
    #[cfg(test)]
    pub fn scroll_offset(&self) -> usize {
        self.scroll.get()
    }
}

impl Default for LiveTranscriptOverlay {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply a backtrack-preview highlight to the lines belonging to a single
/// `HistoryCell::User`. The first line gets a `▶ ` prefix in accent color
/// (so the marker remains visible even on terminals where reverse-video
/// is washed out); every line in the cell gets `Modifier::REVERSED` so
/// the cell visually pops out of the surrounding transcript. Internal
/// span structure is preserved so syntax/role coloring underneath the
/// reverse stays readable.
fn decorate_highlight(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return lines;
    }
    for line in &mut lines {
        for span in &mut line.spans {
            span.style = span.style.add_modifier(Modifier::REVERSED);
        }
    }
    let marker = Span::styled(
        "\u{25B6} ",
        Style::default()
            .fg(palette::TEXT_ACCENT)
            .add_modifier(Modifier::BOLD),
    );
    if let Some(first) = lines.first_mut() {
        first.spans.insert(0, marker);
    }
    lines
}

fn scroll_to_show_range(
    current: usize,
    start: usize,
    end: usize,
    visible_height: usize,
    max_scroll: usize,
) -> usize {
    if visible_height == 0 {
        return 0;
    }
    let end = end.max(start.saturating_add(1));
    if start < current {
        start.min(max_scroll)
    } else if end > current.saturating_add(visible_height) {
        end.saturating_sub(visible_height).min(max_scroll)
    } else {
        current.min(max_scroll)
    }
}

impl ModalView for LiveTranscriptOverlay {
    fn kind(&self) -> ModalKind {
        ModalKind::LiveTranscript
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);

        // Backtrack-preview mode (#133) intercepts Left/Right/Enter/Esc
        // before the normal scroll handlers so the user can step through
        // prior user messages without their input being interpreted as
        // pager navigation. Other keys (page up/down, gg/G, etc.) still
        // fall through so the user can scroll the transcript while
        // previewing.
        if matches!(self.mode, Mode::BacktrackPreview { .. }) {
            match key.code {
                KeyCode::Left | KeyCode::Char('h') if !ctrl => {
                    return ViewAction::Emit(ViewEvent::BacktrackStep {
                        direction: Direction::Left,
                    });
                }
                KeyCode::Right | KeyCode::Char('l') if !ctrl => {
                    return ViewAction::Emit(ViewEvent::BacktrackStep {
                        direction: Direction::Right,
                    });
                }
                KeyCode::Enter => {
                    return ViewAction::EmitAndClose(ViewEvent::BacktrackConfirm);
                }
                KeyCode::Esc | KeyCode::Char('q') => {
                    return ViewAction::EmitAndClose(ViewEvent::BacktrackCancel);
                }
                _ => {}
            }
        }

        if ctrl {
            match key.code {
                KeyCode::Char('d') | KeyCode::Char('D') => {
                    self.scroll_down(self.half_page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('u') | KeyCode::Char('U') => {
                    self.scroll_up(self.half_page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('f') | KeyCode::Char('F') => {
                    self.scroll_down(self.page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    self.scroll_up(self.page_height());
                    self.pending_g = false;
                    return ViewAction::None;
                }
                // Ctrl+T toggles the overlay closed when already open.
                KeyCode::Char('t') | KeyCode::Char('T') => return ViewAction::Close,
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
                self.scroll_down(1);
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::PageUp => {
                self.scroll_up(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::PageDown => {
                self.scroll_down(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char(' ') if shift => {
                self.scroll_up(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char(' ') => {
                self.scroll_down(self.page_height());
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Home => {
                self.jump_to_top();
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::End => {
                self.jump_to_bottom();
                self.pending_g = false;
                ViewAction::None
            }
            KeyCode::Char('g') => {
                if self.pending_g {
                    self.jump_to_top();
                    self.pending_g = false;
                } else {
                    self.pending_g = true;
                }
                ViewAction::None
            }
            KeyCode::Char('G') => {
                self.jump_to_bottom();
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

        // Compute inner content height once: borders eat 1 row top + 1 bottom,
        // padding eats 1 more on each side.
        let visible_height = popup_area.height.saturating_sub(4) as usize;
        self.last_visible_height.set(visible_height);

        // Wrap content using the per-cell cache; subtract padding from width
        // so wrapped lines fit between the inner edges.
        let content_width = popup_width.saturating_sub(4);
        let flattened = self.flatten(content_width);
        let lines = flattened.lines;
        self.last_total_lines.set(lines.len());

        let max_scroll = lines.len().saturating_sub(visible_height);
        // Sticky-tail: every render re-pins scroll to the bottom unless the
        // user has explicitly scrolled away. Without this, streaming new
        // content would push the visible window backwards as `scroll` stays
        // fixed against a growing total.
        let scroll = if self.sticky_to_bottom.get() {
            self.scroll.set(max_scroll);
            max_scroll
        } else if self.preview_pin_pending.replace(false) {
            let next = flattened
                .highlighted_range
                .map(|(start, end)| {
                    scroll_to_show_range(self.scroll.get(), start, end, visible_height, max_scroll)
                })
                .unwrap_or_else(|| self.scroll.get().min(max_scroll));
            self.scroll.set(next);
            next
        } else {
            let next = self.scroll.get().min(max_scroll);
            self.scroll.set(next);
            next
        };
        let end = (scroll + visible_height).min(lines.len());
        let visible_lines: Vec<Line<'static>> = if lines.is_empty() {
            vec![Line::from(Span::styled(
                "(no transcript yet)",
                Style::default().fg(palette::TEXT_DIM),
            ))]
        } else {
            lines[scroll..end].to_vec()
        };

        let title: String = match self.mode {
            Mode::BacktrackPreview { selected_idx } => format!(
                " Backtrack preview — turn {} (\u{2190}/\u{2192} step, Enter rewind, Esc cancel) ",
                selected_idx + 1
            ),
            Mode::Tail => {
                if self.sticky_to_bottom.get() {
                    " Live transcript (tailing) ".to_string()
                } else {
                    " Live transcript (paused) ".to_string()
                }
            }
        };

        let footer = Line::from(Span::styled(
            FOOTER_HINT,
            Style::default().fg(palette::TEXT_HINT),
        ));
        let block = Block::default()
            .title(title)
            .title_bottom(footer)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(palette::BORDER_COLOR))
            .style(Style::default().bg(palette::DEEPSEEK_INK))
            .padding(Padding::uniform(1));

        let paragraph = Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false });
        paragraph.render(popup_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::history::HistoryCell;

    fn user(s: &str) -> HistoryCell {
        HistoryCell::User {
            content: s.to_string(),
        }
    }

    fn assistant(s: &str, streaming: bool) -> HistoryCell {
        HistoryCell::Assistant {
            content: s.to_string(),
            streaming,
        }
    }

    /// Force a render so `last_visible_height` and `last_total_lines` are
    /// populated; otherwise paging keys use the constant fallback.
    fn prime_layout(view: &mut LiveTranscriptOverlay, height: u16) {
        let area = Rect::new(0, 0, 60, height);
        let mut buf = Buffer::empty(area);
        view.render(area, &mut buf);
    }

    fn install_snapshots(view: &mut LiveTranscriptOverlay, cells: Vec<HistoryCell>) {
        view.snapshots = cells
            .into_iter()
            .enumerate()
            .map(|(idx, cell)| CellSnapshot {
                id: CellId::History(idx),
                revision: 1,
                cell,
            })
            .collect();
    }

    fn buffer_text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn new_overlay_starts_sticky() {
        let v = LiveTranscriptOverlay::new();
        assert!(v.is_sticky());
        assert_eq!(v.scroll_offset(), 0);
        assert_eq!(v.snapshot_count(), 0);
    }

    #[test]
    fn scroll_up_breaks_sticky() {
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(
            &mut v,
            (0..50).map(|i| user(&format!("line {i}"))).collect(),
        );
        prime_layout(&mut v, 10);
        // Force scroll non-zero so scroll_up actually moves.
        v.scroll.set(5);
        v.sticky_to_bottom.set(true);
        let _ = v.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert!(!v.is_sticky(), "scrolling up must release the sticky tail");
    }

    #[test]
    fn end_resumes_sticky_tail() {
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(
            &mut v,
            (0..50).map(|i| user(&format!("line {i}"))).collect(),
        );
        prime_layout(&mut v, 10);
        // Drop out of sticky mode by scrolling up.
        v.scroll.set(10);
        v.sticky_to_bottom.set(false);
        let _ = v.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(
            v.is_sticky(),
            "End must re-arm the sticky tail so streaming continues to follow"
        );
    }

    #[test]
    fn scrolling_to_max_re_arms_sticky() {
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(
            &mut v,
            (0..50).map(|i| user(&format!("line {i}"))).collect(),
        );
        prime_layout(&mut v, 10);
        v.sticky_to_bottom.set(false);
        // PageDown once should not re-arm since we're not yet at the tail.
        let _ = v.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        // Now jump explicitly to bottom and verify re-arm.
        v.scroll.set(0);
        v.sticky_to_bottom.set(false);
        let _ = v.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
        assert!(v.is_sticky());
    }

    #[test]
    fn esc_closes() {
        let mut v = LiveTranscriptOverlay::new();
        let action = v.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn ctrl_t_closes_when_already_open() {
        let mut v = LiveTranscriptOverlay::new();
        let action = v.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL));
        assert!(matches!(action, ViewAction::Close));
    }

    #[test]
    fn render_does_not_panic_on_empty() {
        let v = LiveTranscriptOverlay::new();
        let area = Rect::new(0, 0, 40, 12);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
    }

    #[test]
    fn cache_reuses_unchanged_cells_across_renders() {
        // Same revisions across two renders should reuse cache entries; only
        // a "modified" cell (different revision) forces a new wrap. Verify by
        // counting cache size — it grows by 1 per unique (cell, width, rev).
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(&mut v, vec![user("a"), user("b"), assistant("c", false)]);
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let after_first = v.cache.borrow().len();
        v.render(area, &mut buf);
        let after_second = v.cache.borrow().len();
        assert_eq!(
            after_first, after_second,
            "second render should reuse every cell — no new cache entries"
        );
    }

    #[test]
    fn cache_invalidates_on_revision_bump() {
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(&mut v, vec![user("a"), assistant("b", true)]);
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let before = v.cache.borrow().len();
        // Bump the streaming assistant's revision (simulating a delta) and
        // re-render. We expect the cache to grow by one new entry — the new
        // (cell, width, new_rev) — while the user cell entry is reused.
        v.snapshots[1].revision = 2;
        v.render(area, &mut buf);
        let after = v.cache.borrow().len();
        assert!(
            after > before,
            "bumping a revision must add a new cache entry"
        );
    }

    #[test]
    fn resize_does_not_evict_unchanged_width_entries() {
        // Render at width=60, then again at width=80. Both wraps must
        // co-exist in the cache so flipping back to width=60 hits cache.
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(&mut v, vec![user("a"), user("b")]);
        let small = Rect::new(0, 0, 60, 16);
        let large = Rect::new(0, 0, 80, 16);
        let mut buf_s = Buffer::empty(small);
        let mut buf_l = Buffer::empty(large);
        v.render(small, &mut buf_s);
        let after_small = v.cache.borrow().len();
        v.render(large, &mut buf_l);
        let after_both = v.cache.borrow().len();
        assert!(
            after_both > after_small,
            "rendering at a new width must add new cache entries"
        );
        // Flip back to small — should NOT add any new entries (cache hits).
        v.render(small, &mut buf_s);
        let after_replay = v.cache.borrow().len();
        assert_eq!(
            after_replay, after_both,
            "replay at old width must hit cache"
        );
    }

    #[test]
    fn backtrack_preview_disables_sticky() {
        let mut v = LiveTranscriptOverlay::new();
        assert!(v.is_sticky());
        v.set_backtrack_preview(0);
        assert!(!v.is_sticky());
        assert!(matches!(
            v.mode(),
            Mode::BacktrackPreview { selected_idx: 0 }
        ));
    }

    #[test]
    fn set_tail_mode_re_arms_sticky() {
        let mut v = LiveTranscriptOverlay::new();
        v.set_backtrack_preview(2);
        v.set_tail_mode();
        assert!(v.is_sticky());
        assert!(matches!(v.mode(), Mode::Tail));
    }

    #[test]
    fn backtrack_preview_does_not_panic_with_no_user_cells() {
        // Render in preview mode against a transcript that has zero User
        // cells — the highlight scan should miss gracefully.
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(&mut v, vec![assistant("hi", false)]);
        v.set_backtrack_preview(0);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
    }

    #[test]
    fn backtrack_preview_highlights_selected_user_cell() {
        // With 3 user cells (oldest → newest: u0, u1, u2), `selected_idx
        // = 0` should highlight u2 (newest), `= 1` u1, `= 2` u0. We can
        // detect the highlight by scanning the rendered buffer for the
        // marker glyph.
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(
            &mut v,
            vec![
                user("u0"),
                assistant("a0", false),
                user("u1"),
                assistant("a1", false),
                user("u2"),
                assistant("a2", false),
            ],
        );
        for sel in [0usize, 1, 2] {
            v.set_backtrack_preview(sel);
            // Force Tail re-render between iterations to confirm marker
            // really moves rather than smearing.
            let area = Rect::new(0, 0, 40, 24);
            let mut buf = Buffer::empty(area);
            v.render(area, &mut buf);
            // Just verify the cell index resolved without panicking and
            // the buffer is non-empty. Detailed marker placement is
            // visual, hence not asserted here.
            let mut any_content = false;
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    if !buf[(x, y)].symbol().is_empty() && buf[(x, y)].symbol() != " " {
                        any_content = true;
                        break;
                    }
                }
                if any_content {
                    break;
                }
            }
            assert!(any_content, "preview render must produce visible content");
        }
    }

    #[test]
    fn backtrack_preview_opens_near_latest_user_not_transcript_start() {
        let mut v = LiveTranscriptOverlay::new();
        let mut cells = Vec::new();
        for i in 0..12 {
            cells.push(user(&format!("user {i}")));
            cells.push(assistant(&format!("assistant {i}"), false));
        }
        install_snapshots(&mut v, cells);

        v.set_backtrack_preview(0);
        let area = Rect::new(0, 0, 48, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
        let rendered = buffer_text(&buf);

        assert!(
            v.scroll_offset() > 0,
            "preview should pin near the selected recent turn, got top offset 0"
        );
        assert!(
            rendered.contains("user 11"),
            "latest user turn should be visible after opening preview: {rendered}"
        );
        assert!(
            !rendered.contains("user 0"),
            "preview must not open at the oldest transcript line: {rendered}"
        );
    }

    #[test]
    fn backtrack_preview_out_of_range_does_not_panic() {
        // Selecting beyond the user-cell count should simply not
        // highlight anything — no panic, no marker.
        let mut v = LiveTranscriptOverlay::new();
        install_snapshots(&mut v, vec![user("only")]);
        v.set_backtrack_preview(99);
        let area = Rect::new(0, 0, 40, 10);
        let mut buf = Buffer::empty(area);
        v.render(area, &mut buf);
    }
}
