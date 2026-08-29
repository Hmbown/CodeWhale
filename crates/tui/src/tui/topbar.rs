//! Tideline topbar — the one-row status surface from the approved screens.
//!
// Translation scaffolding: the widget is proven against golden buffers here
// and is wired into the live shell by the topbar landing slice (spec §3,
// `docs/design/TIDELINE_RATATUI_TRANSLATION.md`). Until that slice lands it
// has no production caller, so dead-code is expected, not a defect.
#![allow(dead_code)]
//!
//! This is the translation seam between the approved Tideline reference
//! screens and the live shell. It is a pure, deterministic widget: no `App`,
//! no wall-clock read (the clock string is injected), no ambient motion. The
//! caller owns facts; this module owns cells.
//!
//! Segment grammar (left → right): brand lockup, then contextual segments as
//! `label value` pairs joined by `│`, then the pinned right side — context
//! meter and clock. Segments shed in a declared order as width drops; brand,
//! context meter, and clock are the guaranteed floor (spec §5b shed order).
//!
//! Clickability: every segment records a `Rect` hitbox so `mouse_ui` can map
//! a click to its action (spec §5a). Hitboxes are collected through
//! [`render_topbar`] into any sink — the live shell stores them on
//! `ViewportState` exactly like `last_workflow_panel_area`.
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

/// The three-cell fluke projection of the approved brand asset (spec §2:
/// SVG is not renderable; the cell projection is the contract, ASCII
/// `<.>` under ascii-safe mode).
pub const FLUKE: &str = "▚△▞";
pub const FLUKE_ASCII: &str = "<.>";

/// Separator between segments — one cell, dim.
const SEGMENT_JOIN: &str = " │ ";
/// Gap between the brand lockup and the first segment.
const BRAND_GAP: &str = "   ";
/// Width of the context meter bar (cells of ▰/▱).
const METER_CELLS: usize = 5;

/// Identity of a clickable topbar segment. One variant per action the
/// approved screens expose (spec §5a: every segment has an owner action).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopbarSegmentId {
    /// The brand lockup — click opens the product menu.
    Brand,
    /// Workspace path (startup) — click opens the workspace details.
    Workspace,
    /// Current run (work screen) — click opens the run dashboard.
    Run,
    /// Active pod (work screen) — click opens the pod ledger.
    Pod,
    /// Whale capacity `n/m` (work screen) — click opens the pod roster.
    Whales,
    /// Effective model / route — click opens the provider inspector.
    Model,
    /// Theme name (startup) — click opens the theme picker.
    Theme,
    /// Settings breadcrumb (settings screen) — click walks up one category.
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
    /// Segment under the mouse (hover affordance: value brightens).
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

fn fluke_of(ascii_safe: bool) -> &'static str {
    if ascii_safe { FLUKE_ASCII } else { FLUKE }
}

fn brand_width(ascii_safe: bool) -> usize {
    fluke_of(ascii_safe).width() + 1 + "CODEWHALE".width()
}

fn meter_ink_for(pct: u8) -> ChromeInk {
    if pct >= 80 {
        ChromeInk::Attention
    } else {
        ChromeInk::Info
    }
}

impl Widget for Topbar<'_> {
    #[allow(clippy::too_many_lines)]
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 || area.width < 1 {
            return;
        }
        let theme = self.theme;
        let ascii = self.ascii_safe;

        // Right-side pinned block: `context NN% ▰▰▱▱▱  27 Aug 2026 14:42:18`.
        let pct = self.context_percent.clamp(0, 100);
        let meter: String = (0..METER_CELLS)
            .map(|i| {
                let filled = (i + 1) * 100 / METER_CELLS <= usize::from(pct);
                sym(if filled { "▰" } else { "▱" }, ascii)
            })
            .collect();
        let meter_ink = meter_ink_for(pct);
        let clock_text = self.clock;
        let right_text = format!("context {}% {}  {}", pct, meter, clock_text);
        let mut right_width = right_text.width();

        // Shed pass: drop segments (highest shed_priority first) and finally
        // shorten the clock until the row fits. Brand + context + clock are
        // the floor; if even the floor cannot fit, the clock sheds to time
        // only, then the right block wins by truncation from the left.
        let brand_w = brand_width(ascii);
        let join_w = SEGMENT_JOIN.width();
        let mut kept: Vec<&TopbarSegment> = self.segments.iter().collect();
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

        let mut x = area.x as usize;
        let y = area.y;
        // All positions below are usize and cast at the `set_span` boundary;
        // every write is clamped inside `area` by construction.
        let set = |buf: &mut Buffer, cx: usize, span: &Span<'_>| {
            buf.set_span(cx as u16, y, span, span.content.width() as u16);
        };

        // Brand lockup: fluke in Attention gold (the whale mark is the one
        // gold that is not chrome, per the token table), wordmark bold.
        let fluke = fluke_of(ascii);
        set(
            buf,
            x,
            &Span::styled(fluke, chrome(theme, ChromeInk::Attention)),
        );
        x += fluke.width();
        let wordmark = " CODEWHALE";
        set(
            buf,
            x,
            &Span::styled(
                wordmark,
                chrome(theme, ChromeInk::Attention).add_modifier(Modifier::BOLD),
            ),
        );
        x += wordmark.width();
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
            let hovered = self.hovered == Some(segment.id);
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
        let shown_clock = if dropped_clock_prefix {
            &self.clock[self.clock.len() - 8..]
        } else {
            self.clock
        };
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
/// The brand lockup and the pinned right block are hitboxes too (brand opens
/// the product menu; context opens the context inspector) — returned first
/// and last respectively.
#[must_use]
pub fn topbar_hitboxes(topbar: &Topbar<'_>, area: Rect) -> Vec<TopbarHitbox> {
    let mut out = Vec::new();
    if area.height < 1 || area.width < 1 {
        return out;
    }
    let ascii = topbar.ascii_safe;
    let brand_w = brand_width(ascii);
    out.push(TopbarHitbox {
        id: TopbarSegmentId::Brand,
        area: Rect {
            x: area.x,
            y: area.y,
            width: brand_w as u16,
            height: 1,
        },
    });
    let mut x = area.x as usize + brand_w + BRAND_GAP.width();
    let join_w = SEGMENT_JOIN.width();
    for (index, segment) in topbar.segments.iter().enumerate() {
        if index > 0 {
            x += join_w;
        }
        let w = segment.rendered_width();
        out.push(TopbarHitbox {
            id: segment.id,
            area: Rect {
                x: x as u16,
                y: area.y,
                width: w as u16,
                height: 1,
            },
        });
        x += w;
    }
    out
}

#[cfg(test)]
mod tests;
