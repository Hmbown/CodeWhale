//! Tideline receipt stream rendering (spec §5a). A pure, deterministic
//! widget over injected events — the caller projects the transcript.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme, chrome_style};

/// Typed receipt state — the marks from the §1 cell inventory, each with a
/// word so color never invents state (§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub enum TidelineReceiptState {
    /// `●` working — Active ink; echolocation rides the live row only.
    Working,
    /// `○` ready — available but not current.
    Ready,
    /// `✓` done — Outcome ink.
    Done,
    /// `!` caution — Attention ink plus a word.
    Caution,
    /// `✗` failed — Failure ink plus a word.
    Failed,
}

impl TidelineReceiptState {
    #[must_use]
    pub fn mark(self) -> &'static str {
        match self {
            Self::Working => "●",
            Self::Ready => "○",
            Self::Done => "✓",
            Self::Caution => "!",
            Self::Failed => "✗",
        }
    }

    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Working => "working",
            Self::Ready => "ready",
            Self::Done => "done",
            Self::Caution => "caution",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn ink(self) -> ChromeInk {
        match self {
            Self::Working => ChromeInk::Active,
            Self::Ready => ChromeInk::MetadataDim,
            Self::Done => ChromeInk::Outcome,
            Self::Caution => ChromeInk::Attention,
            Self::Failed => ChromeInk::Failure,
        }
    }
}

/// One stream event in display order.
#[derive(Debug, Clone)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub enum TidelineStreamEvent {
    /// Human turn — `▎` user mark.
    UserTurn { text: String },
    /// Assistant turn — transcript rail continuation.
    AssistantTurn { text: String },
    /// Fleet-formation tree (`├──`/`└──` edges) — the same object the ledger
    /// below shows; continuity is the design (§7 orient moment).
    PodFormation { edges: Vec<(bool, String)> },
    /// State-marked receipt row: mark + label + timestamp + receipt count.
    Receipt {
        state: TidelineReceiptState,
        label: String,
        at: String,
        count: Option<usize>,
    },
    /// Indented conclusion block.
    Conclusion { text: String },
}

/// What the caller owes the stream render.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineStream<'a> {
    pub theme: &'a UiTheme,
    pub events: &'a [TidelineStreamEvent],
    /// Focused row index (↑/↓ move it, Enter inspects).
    pub selected: Option<usize>,
    pub ascii_safe: bool,
}

#[allow(dead_code)] // translation scaffolding: builder methods feed tests + the landing slice
impl<'a> TidelineStream<'a> {
    #[allow(dead_code)] // translation scaffolding: wired by the landing slice
    #[must_use]
    pub fn new(theme: &'a UiTheme, events: &'a [TidelineStreamEvent]) -> Self {
        Self {
            theme,
            events,
            selected: None,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn selected(mut self, selected: Option<usize>) -> Self {
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

fn schrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    chrome_style(theme, ink)
}

fn sput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

fn struncate(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            break;
        }
        out.push(ch);
        used += w;
    }
    out
}

/// Paint the receipt stream. The legend row is the last line and teaches
/// the marks in place (§7). The fleet-formation tree draws all edges as one
/// still frame — the ≤600 ms top-down reveal is a landing-slice motion.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn render_tideline_stream(area: Rect, buf: &mut Buffer, stream: &TidelineStream<'_>) {
    if area.width < 10 || area.height < 1 {
        return;
    }
    let theme = stream.theme;
    let width = area.width as usize;
    let legend_reserve = if area.height > 2 { 1 } else { 0 };
    let body_height = area.height - legend_reserve;

    let mut y = area.y;
    let bottom = area.y + body_height;
    for (row_index, event) in stream.events.iter().enumerate() {
        if y >= bottom {
            break;
        }
        let selected = stream.selected == Some(row_index);
        let mut advance = 1;
        match event {
            TidelineStreamEvent::UserTurn { text } => {
                let mark = stream.sym("▎");
                sput(buf, area.x, y, &mark, schrome(theme, ChromeInk::Identity));
                let line = struncate(&stream.sym(text), width.saturating_sub(2));
                sput(
                    buf,
                    area.x + 2,
                    y,
                    &line,
                    schrome(theme, ChromeInk::MetadataValue).add_modifier(Modifier::BOLD),
                );
            }
            TidelineStreamEvent::AssistantTurn { text } => {
                let rail = stream.sym("▏ ");
                sput(
                    buf,
                    area.x,
                    y,
                    &rail,
                    schrome(theme, ChromeInk::MetadataDim),
                );
                let line = struncate(&stream.sym(text), width.saturating_sub(2));
                sput(
                    buf,
                    area.x + 2,
                    y,
                    &line,
                    schrome(theme, ChromeInk::MetadataValue),
                );
            }
            TidelineStreamEvent::PodFormation { edges } => {
                for (position, (is_last, edge)) in edges.iter().enumerate() {
                    let ey = y + position as u16;
                    if ey >= bottom {
                        break;
                    }
                    let branch = if *is_last { "└── " } else { "├── " };
                    let line = struncate(
                        &stream.sym(&format!("{branch}{edge}")),
                        width.saturating_sub(1),
                    );
                    sput(buf, area.x + 1, ey, &line, schrome(theme, ChromeInk::Info));
                }
                advance = edges.len().max(1) as u16;
            }
            TidelineStreamEvent::Receipt {
                state,
                label,
                at,
                count,
            } => {
                sput(
                    buf,
                    area.x,
                    y,
                    &stream.sym(state.mark()),
                    schrome(theme, state.ink()),
                );
                let count_text = count.map(|n| format!(" ×{n}")).unwrap_or_default();
                let line = struncate(
                    &stream.sym(&format!("{} {label} · {at}{count_text}", state.word())),
                    width.saturating_sub(2),
                );
                let mut style = schrome(theme, state.ink());
                if selected {
                    style = style.add_modifier(Modifier::BOLD);
                    // The marker needs its own column left of the mark; at
                    // the stream's left edge the state mark wins (a marker
                    // must never erase state) and bold carries the focus.
                    if area.x > 0 {
                        sput(
                            buf,
                            area.x - 1,
                            y,
                            &stream.sym("▸"),
                            schrome(theme, ChromeInk::Identity),
                        );
                    }
                }
                sput(buf, area.x + 2, y, &line, style);
            }
            TidelineStreamEvent::Conclusion { text } => {
                let line = struncate(&stream.sym(text), width.saturating_sub(4));
                sput(
                    buf,
                    area.x + 3,
                    y,
                    &line,
                    schrome(theme, ChromeInk::Outcome),
                );
            }
        }
        y += advance;
    }

    // Legend row — the marks taught in place.
    if legend_reserve == 1 {
        let legend = "● working   ○ ready   ✓ done   ! caution   ✗ failed";
        let legend = struncate(&stream.sym(legend), width);
        sput(
            buf,
            area.x,
            area.y + area.height - 1,
            &legend,
            schrome(theme, ChromeInk::MetadataHint),
        );
    }
}

/// Row hitboxes for the stream (transcript click path, spec §6): one rect
/// per event, fleet trees spanning their edges.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_stream_hitboxes(area: Rect, stream: &TidelineStream<'_>) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.width < 10 || area.height < 1 {
        return out;
    }
    let legend_reserve = if area.height > 2 { 1 } else { 0 };
    let body_height = area.height - legend_reserve;
    let mut y = area.y;
    let bottom = area.y + body_height;
    for event in stream.events {
        if y >= bottom {
            break;
        }
        let height = match event {
            TidelineStreamEvent::PodFormation { edges } => edges.len().max(1) as u16,
            _ => 1,
        };
        if y + height <= bottom {
            out.push(Rect {
                x: area.x,
                y,
                width: area.width,
                height,
            });
        }
        y += height;
    }
    out
}
