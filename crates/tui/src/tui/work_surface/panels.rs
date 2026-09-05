//! Tideline fleet ledger (spec §2 ledger resolution, §5a "Fleet ledger", §5b
//! ledger columns). The line-list panel path that used to live here
//! (Context as a fact list with nothing to click) is gone: every dock view
//! now renders through the row/hitbox machinery in `render/`, so a context
//! fact is a row like any other and the `compact now` row is a door.

// The whale table with fixed columns and per-column truncation — never wrap.
// Replaces the workflow-panel duplicate as the one whale surface. Translation
// scaffolding in the topbar mold: pure, deterministic, injected rows (the
// caller projects `subagent_cache` + worker runtime states); wired into the
// work stage at the landing slice (#5698 gate).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme, chrome_style};

/// WHALE column width — names are short by contract and never truncate.
const WHALE_CELLS: usize = 10;
/// STATE column width — glyph + word, e.g. `● running`.
const STATE_CELLS: usize = 12;
/// Time-column widths (ELAPSED, RECEIPTS, LAST UPDATE).
const TIME_CELLS: usize = 8;

/// Whale runtime state for the ledger row (§5d marks table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub enum TidelineWhaleState {
    /// ● working — Active ink.
    Working,
    /// ✓ done — Outcome ink.
    Done,
    /// ! caution — Attention ink (color never invents state; the word says it).
    Caution,
    /// ✗ failed — Failure ink (red stays failure-only).
    Failed,
    /// ○ idle — ready but not active.
    Idle,
}

impl TidelineWhaleState {
    /// Glyph + word within STATE_CELLS.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "● working",
            Self::Done => "✓ done",
            Self::Caution => "! caution",
            Self::Failed => "✗ failed",
            Self::Idle => "○ idle",
        }
    }

    #[must_use]
    pub fn ink(self) -> ChromeInk {
        match self {
            Self::Working => ChromeInk::Active,
            Self::Done => ChromeInk::Outcome,
            Self::Caution => ChromeInk::Attention,
            Self::Failed => ChromeInk::Failure,
            Self::Idle => ChromeInk::MetadataDim,
        }
    }
}

/// One ledger row. Every field already formatted by the caller (elapsed,
/// receipts count, HH:MM:SS clock) so renders stay deterministic.
#[derive(Debug, Clone)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineLedgerRow {
    /// Whale name, ≤10 cells by contract — never truncated.
    pub whale: String,
    /// Assignment objective; truncated with `…`, never wrapped.
    pub assignment: String,
    pub state: TidelineWhaleState,
    /// Elapsed label, e.g. `1m 15s`.
    pub elapsed: String,
    /// Receipt count label, e.g. `12`.
    pub receipts: String,
    /// Last-update clock `HH:MM:SS`.
    pub last_update: String,
}

/// The visible column set for a main-area width (spec §5b): time columns
/// shed LAST UPDATE → RECEIPTS → ELAPSED before ASSIGNMENT loses cells; at
/// 80 columns the ledger is WHALE │ ASSIGNMENT │ STATE.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineLedgerColumns {
    pub elapsed: bool,
    pub receipts: bool,
    pub last_update: bool,
}

#[allow(dead_code)] // translation scaffolding: builder/convenience methods feed tests + the landing slice
impl TidelineLedgerColumns {
    #[must_use]
    pub fn for_width(width: u16) -> Self {
        if width >= 130 {
            Self {
                elapsed: true,
                receipts: true,
                last_update: true,
            }
        } else if width >= 110 {
            Self {
                elapsed: true,
                receipts: true,
                last_update: false,
            }
        } else {
            Self {
                elapsed: false,
                receipts: false,
                last_update: false,
            }
        }
    }

    /// Column headers in display order.
    #[must_use]
    pub fn headers(self) -> Vec<&'static str> {
        let mut out = vec!["WHALE", "ASSIGNMENT", "STATE"];
        if self.elapsed {
            out.push("ELAPSED");
        }
        if self.receipts {
            out.push("RECEIPTS");
        }
        if self.last_update {
            out.push("LAST UPDATE");
        }
        out
    }
}

/// What the caller owes the ledger render.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelinePodLedger<'a> {
    pub theme: &'a UiTheme,
    pub rows: &'a [TidelineLedgerRow],
    /// Selected row — `▶` marker; Enter/click inspects beside the evidence.
    pub selected: usize,
    pub ascii_safe: bool,
}

#[allow(dead_code)] // translation scaffolding: builder methods feed tests + the landing slice
impl<'a> TidelinePodLedger<'a> {
    #[allow(dead_code)] // translation scaffolding: wired by the landing slice
    #[must_use]
    pub fn new(theme: &'a UiTheme, rows: &'a [TidelineLedgerRow]) -> Self {
        Self {
            theme,
            rows,
            selected: 0,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    fn sym(&self, glyph: &str) -> String {
        if !self.ascii_safe {
            return glyph.to_string();
        }
        if let Some(fb) = crate::tui::glyphs::ascii_fallback(glyph) {
            return fb.to_string();
        }
        glyph
            .chars()
            .map(|c| {
                crate::tui::glyphs::ascii_fallback(&c.to_string())
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            })
            .collect()
    }
}

fn lchrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    chrome_style(theme, ink)
}

fn lput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

fn ltruncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let ellipsis = "…";
    let mut out = String::new();
    let mut used = 0;
    let budget = width.saturating_sub(1);
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

/// Paint the fleet ledger: `FLEET LEDGER` title, column header row, one-line
/// rows (truncate, never wrap) with the selected-row `▶` marker.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn render_tideline_ledger(area: Rect, buf: &mut Buffer, ledger: &TidelinePodLedger<'_>) {
    if area.width < 30 || area.height < 2 {
        return;
    }
    let theme = ledger.theme;
    let columns = TidelineLedgerColumns::for_width(area.width);

    lput(
        buf,
        area.x,
        area.y,
        "FLEET LEDGER",
        lchrome(theme, ChromeInk::Metadata).add_modifier(Modifier::BOLD),
    );

    // Column x positions: marker col (2) then WHALE │ ASSIGNMENT │ STATE [│ time…].
    let sep = ledger.sym("│");
    let sep_w = sep.width() as u16;
    let mut x = area.x + 2;
    let whale_x = x;
    x += WHALE_CELLS as u16 + sep_w;
    let assignment_x = x;
    let assignment_w = {
        let mut w = area
            .width
            .saturating_sub(2 + (WHALE_CELLS as u16) + sep_w + (STATE_CELLS as u16) + 2 * sep_w);
        if columns.elapsed {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        if columns.receipts {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        if columns.last_update {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        w
    };
    x += assignment_w + sep_w;
    let state_x = x;

    // Header row.
    let header_y = area.y + 1;
    lput(
        buf,
        whale_x,
        header_y,
        "WHALE",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    paint_sep(buf, ledger, theme, whale_x + WHALE_CELLS as u16, header_y);
    lput(
        buf,
        assignment_x,
        header_y,
        "ASSIGNMENT",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    paint_sep(buf, ledger, theme, assignment_x + assignment_w, header_y);
    lput(
        buf,
        state_x,
        header_y,
        "STATE",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    let mut hx = state_x + STATE_CELLS as u16;
    if columns.elapsed {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            "ELAPSED",
            lchrome(theme, ChromeInk::MetadataDim),
        );
        hx += TIME_CELLS as u16;
    }
    if columns.receipts {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            "RECEIPTS",
            lchrome(theme, ChromeInk::MetadataDim),
        );
        hx += TIME_CELLS as u16;
    }
    if columns.last_update {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            &ltruncate("LAST UPDATE", TIME_CELLS),
            lchrome(theme, ChromeInk::MetadataDim),
        );
    }

    // Rows: one line each, selected marker `▶`.
    for (index, row) in ledger.rows.iter().enumerate() {
        let y = area.y + 2 + index as u16;
        if y >= area.y + area.height {
            break;
        }
        let selected = ledger.selected == index;
        if selected {
            lput(
                buf,
                area.x,
                y,
                &ledger.sym("▶"),
                lchrome(theme, ChromeInk::Identity),
            );
        }
        lput(
            buf,
            whale_x,
            y,
            &ledger.sym(&row.whale),
            lchrome(theme, ChromeInk::Identity),
        );
        paint_sep(buf, ledger, theme, whale_x + WHALE_CELLS as u16, y);
        lput(
            buf,
            assignment_x,
            y,
            &ltruncate(&ledger.sym(&row.assignment), assignment_w as usize),
            lchrome(theme, ChromeInk::MetadataValue),
        );
        paint_sep(buf, ledger, theme, assignment_x + assignment_w, y);
        lput(
            buf,
            state_x,
            y,
            &ledger.sym(row.state.label()),
            lchrome(theme, row.state.ink()),
        );
        let mut tx = state_x + STATE_CELLS as u16;
        if columns.elapsed {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.elapsed),
                lchrome(theme, ChromeInk::Metadata),
            );
            tx += TIME_CELLS as u16;
        }
        if columns.receipts {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.receipts),
                lchrome(theme, ChromeInk::Metadata),
            );
            tx += TIME_CELLS as u16;
        }
        if columns.last_update {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.last_update),
                lchrome(theme, ChromeInk::MetadataHint),
            );
        }
    }
}

fn paint_sep(buf: &mut Buffer, ledger: &TidelinePodLedger<'_>, theme: &UiTheme, x: u16, y: u16) {
    let sep = ledger.sym("│");
    lput(buf, x, y, &sep, lchrome(theme, ChromeInk::MetadataDim));
}

/// Row hitboxes → inspector (spec §6): one rect per visible row.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_ledger_hitboxes(area: Rect, ledger: &TidelinePodLedger<'_>) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.width < 30 || area.height < 2 {
        return out;
    }
    for index in 0..ledger.rows.len() {
        let y = area.y + 2 + index as u16;
        if y >= area.y + area.height {
            break;
        }
        out.push(Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        });
    }
    out
}

#[cfg(test)]
mod tideline_tests;
