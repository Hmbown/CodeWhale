//! Tideline topbar — the one-row status surface from the approved screens.
//!
//! This is the translation seam between the approved Tideline reference
//! screens and the live shell. It is a pure, deterministic widget: no `App`,
//! no wall-clock read (the clock string is injected), no ambient motion. The
//! caller owns facts; this module owns cells.
//!
//! The brand lockup is the `CODEWHALE` wordmark alone, in the sanctioned
//! whale-mark gold. There is no glyph before it by founder decree: the
//! canonical mark is a raster asset with no approved ASCII or block-glyph
//! substitute, and a one-row topbar cannot carry it faithfully. The retired
//! hand-drawn crown glyph is absent from this module.
//!
//! Segment grammar (left → right): brand lockup, then contextual segments as
//! `label value` pairs joined by `│`, then the pinned right side — context
//! meter and clock. Segments shed in a declared order as width drops; brand,
//! context meter, and clock are the guaranteed floor (spec §5b shed order).
//!
//! Interaction: segment geometry is recorded for parity tests, but only the
//! effective model/route segment and the pinned context meter advertise an
//! action in the live shell. Status-only facts do not brighten on hover or
//! pretend to be controls.
//!
//! Color: semantic ink only ([`ChromeInk`]); no hex, per the status-bar color
//! grammar. ASCII-safe mode substitutes every glyph through
//! [`glyphs::ascii_fallback`].

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::Widget,
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme};
use crate::tui::glyphs;

/// Separator between segments — one cell, dim.
const SEGMENT_JOIN: &str = " │ ";
/// Gap between the brand lockup and the first segment.
const BRAND_GAP: &str = "   ";
/// Width of the context meter bar (cells of ▰/▱).
const METER_CELLS: usize = 5;
/// The brand lockup: wordmark only, no glyph (founder decree — see the
/// module docs). Pure ASCII, so it never widens under ascii-safe mode.
const WORDMARK: &str = "CODEWHALE";

/// Identity of a topbar segment. Most variants are status facts; the live
/// shell currently registers an action only for [`Self::Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopbarSegmentId {
    /// The brand lockup — status-only until a product menu exists.
    Brand,
    /// Workspace path (startup).
    Workspace,
    /// Current run (work screen).
    Run,
    /// Active pod (work screen).
    Pod,
    /// Whale capacity `n/m` (work screen).
    Whales,
    /// Effective model / route — click opens the provider inspector.
    Model,
    /// Theme name (startup).
    Theme,
    /// Settings breadcrumb (settings screen) — click walks up one category.
    /// Not constructed by the main shell yet: the settings screen is a
    /// later Tideline slice (spec §5a).
    #[allow(dead_code)]
    SettingsPath,
}

impl TopbarSegmentId {
    /// Shed priority: higher sheds first as width drops. `0` never sheds.
    /// The floor is brand + context meter + clock; among segments, Model and
    /// the settings breadcrumb are the last to go because route identity is
    /// the one fact the user must always be able to read (spec §5b).
    #[must_use]
    pub fn shed_priority(self) -> u8 {
        match self {
            Self::Theme => 5,
            Self::Workspace => 4,
            Self::Whales => 3,
            Self::Pod => 2,
            Self::Run | Self::SettingsPath => 1,
            Self::Model | Self::Brand => 0,
        }
    }
}

/// One contextual topbar segment.
#[derive(Debug, Clone)]
pub struct TopbarSegment {
    pub id: TopbarSegmentId,
    pub label: String,
    pub value: String,
    pub ink: ChromeInk,
}

impl TopbarSegment {
    #[must_use]
    pub fn new(id: TopbarSegmentId, label: &str, value: impl Into<String>, ink: ChromeInk) -> Self {
        Self {
            id,
            label: label.to_string(),
            value: value.into(),
            ink,
        }
    }

    fn rendered_width(&self) -> usize {
        segment_text(self).width()
    }
}

fn segment_text(segment: &TopbarSegment) -> String {
    if segment.label.is_empty() {
        segment.value.clone()
    } else {
        format!("{} {}", segment.label, segment.value)
    }
}

/// What the caller owes the topbar. Everything is injected so renders are
/// deterministic (golden buffers) and wall-clock keyed by the owner, never
/// frame-count keyed (spec §5e).
pub struct Topbar<'a> {
    pub theme: &'a UiTheme,
    /// Full clock string, e.g. `27 Aug 2026 14:42:18`. At narrow widths it
    /// sheds to the time-of-day suffix before any segment sheds.
    pub clock: &'a str,
    /// Context window percentage, 0–100.
    pub context_percent: u8,
    /// Contextual segments in display order.
    pub segments: &'a [TopbarSegment],
    /// Actionable segment under the mouse. Only [`TopbarSegmentId::Model`]
    /// currently advertises hover feedback in the live shell.
    pub hovered: Option<TopbarSegmentId>,
    /// ASCII-safe / NO_COLOR mode: every glyph goes through
    /// [`glyphs::ascii_fallback`].
    pub ascii_safe: bool,
}

impl<'a> Topbar<'a> {
    #[must_use]
    pub fn new(
        theme: &'a UiTheme,
        clock: &'a str,
        context_percent: u8,
        segments: &'a [TopbarSegment],
    ) -> Self {
        Self {
            theme,
            clock,
            context_percent,
            segments,
            hovered: None,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    #[must_use]
    pub fn hovered(mut self, hovered: Option<TopbarSegmentId>) -> Self {
        self.hovered = hovered;
        self
    }
}

fn ascii_of(glyph: &str) -> String {
    if let Some(fb) = glyphs::ascii_fallback(glyph) {
        return fb.to_string();
    }
    glyph
        .chars()
        .map(|c| {
            glyphs::ascii_fallback(&c.to_string())
                .map(str::to_string)
                .unwrap_or_else(|| c.to_string())
        })
        .collect()
}

fn sym(glyph: &str, ascii_safe: bool) -> String {
    if ascii_safe {
        ascii_of(glyph)
    } else {
        glyph.to_string()
    }
}

fn brand_width() -> usize {
    WORDMARK.width()
}

fn meter_ink_for(pct: u8) -> ChromeInk {
    if pct >= 80 {
        ChromeInk::Attention
    } else {
        ChromeInk::Info
    }
}

/// The shed pass's answer: which segments survive at this row width, the
/// effective right-block width (after any clock-prefix shed), and the
/// context meter's own span. Shared by the render and the hitbox
/// computation so the two can never disagree about where the meter
/// painted — the same single-arithmetic discipline the startup stage's
/// `startup_layout` follows.
struct ShedRow<'t> {
    kept: Vec<&'t TopbarSegment>,
    dropped_clock_prefix: bool,
    right_width: usize,
    /// Width of the `context NN% ▰▰▱▱▱` span alone.
    context_width: usize,
    meter: String,
}

fn shed_pass<'t>(topbar: &'t Topbar<'_>, area: Rect) -> ShedRow<'t> {
    let ascii = topbar.ascii_safe;
    // Right-side pinned block: `context NN% ▰▰▱▱▱  27 Aug 2026 14:42:18`.
    let pct = topbar.context_percent.clamp(0, 100);
    let meter: String = (0..METER_CELLS)
        .map(|i| {
            let filled = (i + 1) * 100 / METER_CELLS <= usize::from(pct);
            sym(if filled { "▰" } else { "▱" }, ascii)
        })
        .collect();
    let clock_text = topbar.clock;
    let right_text = format!("context {}% {}  {}", pct, meter, clock_text);
    let mut right_width = right_text.width();

    // Shed pass: drop segments (highest shed_priority first) and finally
    // shorten the clock until the row fits. Brand + context + clock are
    // the floor; if even the floor cannot fit, the clock sheds to time
    // only, then the right block wins by truncation from the left.
    let brand_w = brand_width();
    let join_w = SEGMENT_JOIN.width();
    let mut kept: Vec<&TopbarSegment> = topbar.segments.iter().collect();
    let total_needed = |segs: &[&TopbarSegment], right: usize| -> usize {
        brand_w
            + BRAND_GAP.width()
            + segs.iter().map(|s| s.rendered_width()).sum::<usize>()
            + if segs.is_empty() {
                0
            } else {
                join_w * (segs.len() - 1)
            }
            + 2
            + right
    };
    let mut dropped_clock_prefix = false;
    while total_needed(&kept, right_width) > area.width as usize {
        // 1. shed the highest-priority segment
        if let Some(pos) = kept
            .iter()
            .enumerate()
            .filter(|(_, s)| s.id.shed_priority() > 0)
            .max_by_key(|(_, s)| s.id.shed_priority())
            .map(|(i, _)| i)
        {
            kept.remove(pos);
            continue;
        }
        // 2. shed the clock date prefix (keep `HH:MM:SS`)
        if !dropped_clock_prefix && clock_text.len() > 8 {
            let short = &clock_text[clock_text.len() - 8..];
            right_width = format!("context {}% {}  {}", pct, meter, short).width();
            dropped_clock_prefix = true;
            continue;
        }
        // 3. nothing left to shed — the render truncates from the right
        //    (clock goes first, then meter); the brand never truncates.
        break;
    }
    ShedRow {
        kept,
        dropped_clock_prefix,
        right_width,
        context_width: format!("context {}% {}", pct, meter).width(),
        meter,
    }
}

/// The pinned context meter's hitbox (spec §6: the meter is the chrome
/// row's one always-present inspector target — `Alt+C`'s mouse route).
/// Covers exactly the painted `context NN% ▰▰▱▱▱` span. `None` when the
/// row is too narrow for that span to have painted whole and clear of the
/// brand lockup: a hitbox never claims cells another element paints (the
/// posture-floor discipline the classic header's meter hitbox carried).
#[must_use]
pub fn context_meter_hitbox(topbar: &Topbar<'_>, area: Rect) -> Option<Rect> {
    if area.width < 1 || area.height < 1 {
        return None;
    }
    let shed = shed_pass(topbar, area);
    let start = usize::from(area.width).saturating_sub(shed.right_width);
    if start <= brand_width() + BRAND_GAP.width() || shed.context_width >= usize::from(area.width) {
        return None;
    }
    Some(Rect {
        x: area.x + u16::try_from(start).unwrap_or(u16::MAX),
        y: area.y,
        width: u16::try_from(shed.context_width).unwrap_or(area.width),
        height: 1,
    })
}

impl Widget for Topbar<'_> {
    #[allow(clippy::too_many_lines)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 || area.width < 1 {
            return;
        }
        let theme = self.theme;
        let pct = self.context_percent.clamp(0, 100);
        let meter_ink = meter_ink_for(pct);
        let ShedRow {
            kept,
            dropped_clock_prefix,
            right_width,
            meter,
            ..
        } = shed_pass(&self, area);
        let shown_clock = if dropped_clock_prefix {
            &self.clock[self.clock.len() - 8..]
        } else {
            self.clock
        };

        let mut x = area.x as usize;
        let y = area.y;
        // All positions below are usize and cast at the `set_span` boundary;
        // every write is clamped inside `area` by construction.
        let set = |buf: &mut Buffer, cx: usize, span: &Span<'_>| {
            buf.set_span(cx as u16, y, span, span.content.width() as u16);
        };

        // Brand lockup: the wordmark alone in Attention gold, bold (the
        // whale-mark gold is the one gold that is not chrome, per the token
        // table). No glyph precedes it — founder decree, see module docs.
        set(
            buf,
            x,
            &Span::styled(
                WORDMARK,
                chrome(theme, ChromeInk::Attention).add_modifier(Modifier::BOLD),
            ),
        );
        x += WORDMARK.width();
        x += BRAND_GAP.width();

        // Contextual segments with recorded hitboxes.
        for (index, segment) in kept.iter().enumerate() {
            if index > 0 {
                set(
                    buf,
                    x,
                    &Span::styled(SEGMENT_JOIN, chrome(theme, ChromeInk::MetadataDim)),
                );
                x += SEGMENT_JOIN.width();
            }
            let hovered = segment.id == TopbarSegmentId::Model
                && self.hovered == Some(TopbarSegmentId::Model);
            let mut style = chrome(theme, segment.ink);
            if hovered {
                style = style
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED);
            }
            // label dim, value in the segment's ink (two spans, one hitbox)
            if segment.label.is_empty() {
                set(buf, x, &Span::styled(&segment.value, style));
                x += segment.value.width();
            } else {
                set(
                    buf,
                    x,
                    &Span::styled(&segment.label, chrome(theme, ChromeInk::Metadata)),
                );
                x += segment.label.width() + 1;
                set(buf, x, &Span::styled(&segment.value, style));
                x += segment.value.width();
            }
        }

        // Right pinned block, right-aligned to the area edge.
        let mut sx = (area.x as usize + area.width as usize).saturating_sub(right_width);
        let mut spans: Vec<Span> = Vec::with_capacity(6);
        spans.push(Span::styled("context ", chrome(theme, ChromeInk::Metadata)));
        spans.push(Span::styled(
            format!("{}%", pct),
            chrome(theme, ChromeInk::Info),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(meter.clone(), chrome(theme, meter_ink)));
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            shown_clock,
            chrome(theme, ChromeInk::MetadataHint),
        ));
        for span in &spans {
            set(buf, sx, span);
            sx += span.content.width();
        }
    }
}

fn chrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    crate::palette::grammar::chrome_style(theme, ink)
}

/// Recorded hitboxes for one rendered topbar row. Mirrors the
/// `viewport.last_workflow_cancel_area` storage pattern: render computes the
/// rects, the caller stores them, `mouse_ui` hit-tests against them.
#[derive(Debug, Clone)]
pub struct TopbarHitbox {
    pub id: TopbarSegmentId,
    pub area: Rect,
}

/// Compute the hitbox `Rect` for each kept segment. Must be called with the
/// same inputs as the render so the rects match the painted cells exactly.
/// The brand lockup is included as recorded geometry, though it is status-only
/// in the live shell. The context meter has its own exact hitbox helper.
#[must_use]
pub fn topbar_hitboxes(topbar: &Topbar<'_>, area: Rect) -> Vec<TopbarHitbox> {
    let mut out = Vec::new();
    if area.height < 1 || area.width < 1 {
        return out;
    }
    let brand_w = brand_width();
    if brand_w <= usize::from(area.width) {
        out.push(TopbarHitbox {
            id: TopbarSegmentId::Brand,
            area: Rect {
                x: area.x,
                y: area.y,
                width: brand_w as u16,
                height: 1,
            },
        });
    }
    let mut x = area.x as usize + brand_w + BRAND_GAP.width();
    let join_w = SEGMENT_JOIN.width();
    let shed = shed_pass(topbar, area);
    let right_start = usize::from(area.x + area.width).saturating_sub(shed.right_width);
    for (index, segment) in shed.kept.iter().enumerate() {
        if index > 0 {
            x += join_w;
        }
        let w = segment.rendered_width();
        if x + w <= right_start && x + w <= usize::from(area.x + area.width) {
            out.push(TopbarHitbox {
                id: segment.id,
                area: Rect {
                    x: x as u16,
                    y: area.y,
                    width: w as u16,
                    height: 1,
                },
            });
        }
        x += w;
    }
    out
}

#[cfg(test)]
mod tests;
