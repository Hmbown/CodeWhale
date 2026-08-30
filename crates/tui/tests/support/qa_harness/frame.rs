//! Terminal frame snapshot built from the PTY output stream.
//!
//! Wraps `rio-vt` so tests can feed bytes incrementally and ask
//! questions about the current screen contents (visible text, individual rows,
//! does-it-contain-this).

use std::time::Instant;

use rio_vt::ansi::CursorShape;
use rio_vt::config::colors::{AnsiColor, NamedColor};
use rio_vt::crosswords::formatter::FormatOptions;
use rio_vt::crosswords::pos::Column;
use rio_vt::crosswords::square::{ContentTag, Square, Wide};
use rio_vt::crosswords::style::Style;
use rio_vt::crosswords::{Crosswords, CrosswordsSize};
use rio_vt::event::{VoidListener, WindowId};
use rio_vt::performer::handler::Processor;

/// Terminal cell color, matching the three cases theme QA asserts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

pub struct Frame {
    term: Crosswords<VoidListener>,
    parser: Processor,
    captured_at: Option<Instant>,
}

impl Frame {
    pub fn new(rows: u16, cols: u16) -> Self {
        Self {
            term: Crosswords::new(
                grid_size(rows, cols),
                CursorShape::Block,
                VoidListener,
                WindowId::from(0),
                0,
                0,
            ),
            parser: Processor::default(),
            captured_at: None,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.parser.advance(&mut self.term, bytes);
        self.captured_at = Some(Instant::now());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.term.resize(grid_size(rows, cols));
    }

    pub fn rows(&self) -> u16 {
        self.term.screen_lines() as u16
    }

    pub fn cols(&self) -> u16 {
        self.term.columns() as u16
    }

    /// Full visible screen as a single string with a `\n` between rows.
    pub fn text(&self) -> String {
        self.term.format(FormatOptions::plain())
    }

    /// Single row of the screen, 0-indexed from the top, trimmed at the
    /// right edge. Returns the empty string for out-of-range rows.
    ///
    /// Blank cells still occupy a real terminal column between painted cells,
    /// so they are emitted as spaces; the hidden continuation cell of a wide
    /// glyph is skipped so `界 read` does not become `界  read`.
    pub fn row(&self, y: u16) -> String {
        if y >= self.rows() {
            return String::new();
        }
        let rows = self.term.visible_rows();
        let Some(row) = rows.get(usize::from(y)) else {
            return String::new();
        };
        let cols = usize::from(self.cols());
        let mut out = String::with_capacity(cols);
        for col in 0..cols {
            let square = row[Column(col)];
            if matches!(square.wide(), Wide::Spacer) {
                continue;
            }
            let ch = square.c();
            out.push(if ch == '\u{0}' { ' ' } else { ch });
        }
        out.trim_end().to_string()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.text().contains(needle)
    }

    /// First visible coordinate of `needle`, using terminal display columns.
    pub fn find_text(&self, needle: &str) -> Option<(u16, u16)> {
        for row in 0..self.rows() {
            if let Some(col) = self.find_text_in_row(row, needle) {
                return Some((row, col));
            }
        }
        None
    }

    /// Locate text on one parsed terminal row without collapsing blank cells.
    pub fn find_text_in_row(&self, row: u16, needle: &str) -> Option<u16> {
        if row >= self.rows() || needle.is_empty() {
            return None;
        }
        let rows = self.term.visible_rows();
        let grid_row = rows.get(usize::from(row))?;
        let cols = self.cols();
        for start in 0..cols {
            let mut col = start;
            let mut matched = true;
            for ch in needle.chars() {
                if col >= cols {
                    matched = false;
                    break;
                }
                let contents = square_contents(grid_row[Column(usize::from(col))]);
                let mut encoded = [0_u8; 4];
                let expected: &str = ch.encode_utf8(&mut encoded);
                if if ch == ' ' {
                    !contents.is_empty() && contents.as_str() != " "
                } else {
                    contents.as_str() != expected
                } {
                    matched = false;
                    break;
                }
                let width = unicode_width::UnicodeWidthChar::width(ch)
                    .unwrap_or(0)
                    .max(1);
                let Ok(width) = u16::try_from(width) else {
                    return None;
                };
                col = col.saturating_add(width);
            }
            if matched {
                return Some(start);
            }
        }
        None
    }

    /// Foreground/background colors for one terminal cell. Theme QA uses the
    /// parsed ANSI result rather than trusting a screenshot renderer's own
    /// palette or accessibility environment.
    pub fn colors_at(&self, row: u16, col: u16) -> Option<(Color, Color)> {
        let rows = self.term.visible_rows();
        let grid_row = rows.get(usize::from(row))?;
        if usize::from(col) >= usize::from(self.cols()) {
            return None;
        }
        let styles = self.term.grid.styles();
        Some(square_colors(grid_row[Column(usize::from(col))], styles))
    }

    /// Colors on the first cell whose terminal contents equal `symbol`.
    pub fn first_symbol_colors(&self, symbol: &str) -> Option<(Color, Color)> {
        let rows = self.term.visible_rows();
        let styles = self.term.grid.styles();
        let cols = usize::from(self.cols());
        for grid_row in &rows {
            for col in 0..cols {
                let square = grid_row[Column(col)];
                if square_contents(square).as_str() == symbol {
                    return Some(square_colors(square, styles));
                }
            }
        }
        None
    }

    /// Whether any painted cell carries a 24-bit color. The palette adapter
    /// downgrades every truecolor before it reaches crossterm on terminals
    /// that only advertise 256 or 16 colors, so this is the parsed-ANSI proof
    /// that the capability tier was honored.
    pub fn any_truecolor_cell(&self) -> bool {
        let rows = self.term.visible_rows();
        let styles = self.term.grid.styles();
        let cols = usize::from(self.cols());
        for grid_row in &rows {
            for col in 0..cols {
                let (fg, bg) = square_colors(grid_row[Column(col)], styles);
                if matches!(fg, Color::Rgb(..)) || matches!(bg, Color::Rgb(..)) {
                    return true;
                }
            }
        }
        false
    }

    /// Every distinct character painted on the screen.
    pub fn painted_chars(&self) -> std::collections::BTreeSet<char> {
        self.text().chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// Widest parsed row. rio-vt clips at the right margin, so an overflowing
    /// renderer shows up as wrapped content rather than a long row.
    pub fn max_row_width(&self) -> usize {
        (0..self.rows())
            .map(|y| self.row(y).chars().count())
            .max()
            .unwrap_or(0)
    }

    /// Whether any row of the screen has non-blank content.
    pub fn any_visible_text(&self) -> bool {
        self.text().chars().any(|c| !c.is_whitespace())
    }

    /// Cursor position as (row, col).
    pub fn cursor(&self) -> (u16, u16) {
        let pos = self.term.cursor().pos;
        (
            u16::try_from(pos.row.0.max(0)).unwrap_or(u16::MAX),
            u16::try_from(pos.col.0).unwrap_or(u16::MAX),
        )
    }

    /// Render the screen to a string for diagnostic dumps when an
    /// assertion fails.
    pub fn debug_dump(&self) -> String {
        let (rows, cols) = (self.rows(), self.cols());
        let mut out = String::new();
        out.push_str(&format!(
            "== frame {rows}x{cols} cursor={:?} ==\n",
            self.cursor()
        ));
        for y in 0..rows {
            out.push_str(&format!("{y:>3} | {}\n", self.row(y).trim_end()));
        }
        out
    }
}

fn grid_size(rows: u16, cols: u16) -> CrosswordsSize {
    CrosswordsSize::new(usize::from(cols.max(1)), usize::from(rows.max(1)))
}

fn square_contents(square: Square) -> String {
    if matches!(square.wide(), Wide::Spacer) {
        return String::new();
    }
    match square.c() {
        ' ' | '\u{0}' => String::new(),
        ch => ch.to_string(),
    }
}

fn square_colors(square: Square, styles: &[Style]) -> (Color, Color) {
    match square.content_tag() {
        ContentTag::Codepoint => {
            let style = styles
                .get(square.style_id() as usize)
                .copied()
                .unwrap_or_default();
            (map_color(style.fg), map_color(style.bg))
        }
        ContentTag::BgPalette => (Color::Default, Color::Idx(square.bg_palette_index())),
        ContentTag::BgRgb => {
            let (r, g, b) = square.bg_rgb();
            (Color::Default, Color::Rgb(r, g, b))
        }
    }
}

fn map_color(color: AnsiColor) -> Color {
    match color {
        AnsiColor::Named(NamedColor::Foreground | NamedColor::Background) => Color::Default,
        AnsiColor::Named(named) => {
            let index = named as u32;
            if index < 16 {
                Color::Idx(index as u8)
            } else {
                Color::Default
            }
        }
        AnsiColor::Indexed(index) => Color::Idx(index),
        AnsiColor::Spec(rgb) => Color::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

#[cfg(test)]
mod tests {
    use super::Frame;

    #[test]
    fn row_preserves_unpainted_interior_terminal_columns() {
        let mut frame = Frame::new(1, 12);
        frame.feed(b"read\x1b[6Grunning");

        assert_eq!(frame.row(0), "read running");
    }

    #[test]
    fn row_does_not_expand_wide_glyph_continuation_cells() {
        let mut frame = Frame::new(1, 12);
        frame.feed("界 read".as_bytes());

        assert_eq!(frame.row(0), "界 read");
    }
}
