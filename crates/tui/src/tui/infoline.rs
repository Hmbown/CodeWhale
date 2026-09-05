//! The shell's metrics line — one row of session numbers, painted under the
//! posture bar at the bottom of the screen.
//!
//! It used to be a top bar. The founder's call (SHELL-DESIGN-20260901 §2.0):
//! *"Putting the info at the bottom is a better idea, because then you scroll
//! up and it feels intentional. Move the top/side bar to the bottom."* Then
//! (2026-09-02): *less always-on information* — the repository and branch
//! moved to the launch header and the git bottom view, and the DeepSeek
//! harness session metrics came back on screen in their place.
//!
//! The row, left to right, joined by ` · `:
//!
//! ```text
//! deepseek-v4 · ctx 22% · $0.14 · ttft 400ms · 38 tok/s · ↓ 1.2K      Ctrl+/ help
//! ```
//!
//! The model is the one route fact the user checks before a turn, and it
//! stays clickable to the picker; the context reading stays clickable to the
//! inspector. Both are the floor and never shed. Everything else is a
//! metric: the session cost (the same number `/cost`, the roster and the
//! price widget print), time to first token, output rate and output tokens —
//! live while a turn streams, the last turn's figures when idle, and never
//! blank between turns once a turn has reported them.
//!
//! The context reading is painted here and only here — the posture bar above
//! used to print the same percentage a second time from the same snapshot.
//!
//! Shed order as width drops: `tok/s`, `ttft`, `↓ tokens`, the help hint,
//! then the cost ([`InfoSegmentId::shed_priority`]). The model and `ctx NN%`
//! never shed; below that floor the row clips at its right edge.
//!
//! Interaction: segment geometry is recorded for parity tests, but only the
//! model/route segment and the context reading advertise an action in the
//! live shell. Status-only facts do not brighten on hover or pretend to be
//! controls.
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

/// Separator between items — the row's one piece of punctuation.
const ITEM_JOIN: &str = " · ";
/// Minimum gap between the last left item and the pinned help hint.
const HELP_GAP: usize = 2;

/// Identity of a metrics-line segment. The live shell registers an action
/// for [`Self::Model`] (the provider picker) and [`Self::Context`] (the
/// context inspector); the rest are readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoSegmentId {
    /// Effective model / route — click opens the provider picker.
    Model,
    /// Context window reading `ctx NN%` — click opens the inspector.
    Context,
    /// Session cost, the one price number (`$0.14`).
    Cost,
    /// Output tokens of the live or last turn (`↓ 1.2K`).
    OutputTokens,
    /// Time to first token (`ttft 400ms`).
    Ttft,
    /// Output rate (`38 tok/s`).
    Rate,
    /// Prompt cache hit percent (`cache 85%`).
    Cache,
}

impl InfoSegmentId {
    /// Shed priority: higher sheds first as width drops. `0` never sheds.
    /// Segments at or above [`Self::SHED_BEFORE_HELP`] go before the help
    /// hint; the cost outlives the hint because it is the one number that
    /// must keep matching `/cost`, the roster and the price widget.
    #[must_use]
    pub fn shed_priority(self) -> u8 {
        match self {
            Self::Rate => 9,
            Self::Cache => 8,
            Self::Ttft => 8,
            Self::OutputTokens => 7,
            Self::Cost => 6,
            Self::Model | Self::Context => 0,
        }
    }

    /// Priorities at or above this shed before the help hint does.
    const SHED_BEFORE_HELP: u8 = 7;
}

/// One metrics-line segment.
#[derive(Debug, Clone)]
pub struct InfoSegment {
    pub id: InfoSegmentId,
    pub label: String,
    pub value: String,
    pub ink: ChromeInk,
}

impl InfoSegment {
    #[must_use]
    pub fn new(id: InfoSegmentId, label: &str, value: impl Into<String>, ink: ChromeInk) -> Self {
        Self {
            id,
            label: label.to_string(),
            value: value.into(),
            ink,
        }
    }

    fn rendered_width(&self, ascii_safe: bool) -> usize {
        segment_text(self, ascii_safe).width()
    }
}

fn segment_text(segment: &InfoSegment, ascii_safe: bool) -> String {
    if segment.label.is_empty() {
        segment.value.clone()
    } else {
        format!("{} {}", sym(&segment.label, ascii_safe), segment.value)
    }
}

/// What the caller owes the metrics line. Everything is injected so renders
/// are deterministic (golden buffers) and wall-clock keyed by the owner,
/// never frame-count keyed (spec §5e).
pub struct InfoLine<'a> {
    pub theme: &'a UiTheme,
    /// The single right-hand key hint, e.g. `Ctrl+/ help`. Empty means the
    /// caller has no hint to advertise.
    pub help_hint: &'a str,
    /// Segments in display order.
    pub segments: &'a [InfoSegment],
    /// Actionable segment under the mouse. [`InfoSegmentId::Model`] and
    /// [`InfoSegmentId::Context`] advertise hover feedback in the live
    /// shell; both own a click action (picker / inspector).
    pub hovered: Option<InfoSegmentId>,
    /// ASCII-safe / NO_COLOR mode: every glyph goes through
    /// [`glyphs::ascii_fallback`].
    pub ascii_safe: bool,
}

impl<'a> InfoLine<'a> {
    #[must_use]
    pub fn new(theme: &'a UiTheme, help_hint: &'a str, segments: &'a [InfoSegment]) -> Self {
        Self {
            theme,
            help_hint,
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
    pub fn hovered(mut self, hovered: Option<InfoSegmentId>) -> Self {
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

/// The shed pass's answer: which segments survive at this row width and
/// whether the help hint survived. Shared by the render and the hitbox
/// computation so the two can never disagree about the cells a segment
/// painted.
struct ShedRow<'t> {
    kept: Vec<&'t InfoSegment>,
    show_help: bool,
}

fn shed_pass<'t>(info: &'t InfoLine<'_>, area: Rect) -> ShedRow<'t> {
    let ascii = info.ascii_safe;
    let help = sym(info.help_hint, ascii);
    let join_w = sym(ITEM_JOIN, ascii).width();
    let mut kept: Vec<&InfoSegment> = info.segments.iter().collect();
    let left_width = |segs: &[&InfoSegment]| -> usize {
        segs.iter().map(|s| s.rendered_width(ascii)).sum::<usize>()
            + join_w * segs.len().saturating_sub(1)
    };
    let total_needed = |left: usize, show_help: bool| -> usize {
        left + if show_help && !help.is_empty() {
            HELP_GAP + help.width()
        } else {
            0
        }
    };
    // The highest-priority shedding segment, restricted to `min_priority`
    // and above, if any.
    let sheddable = |kept: &[&InfoSegment], min_priority: u8| -> Option<usize> {
        kept.iter()
            .enumerate()
            .filter(|(_, s)| s.id.shed_priority() >= min_priority.max(1))
            .max_by_key(|(_, s)| s.id.shed_priority())
            .map(|(i, _)| i)
    };

    let mut show_help = !help.is_empty();
    while total_needed(left_width(&kept), show_help) > area.width as usize {
        if let Some(pos) = sheddable(&kept, InfoSegmentId::SHED_BEFORE_HELP) {
            kept.remove(pos);
        } else if show_help {
            show_help = false;
        } else if let Some(pos) = sheddable(&kept, 1) {
            kept.remove(pos);
        } else {
            break;
        }
    }

    ShedRow { kept, show_help }
}

/// The context reading's hitbox (spec §6: the reading is the chrome row's one
/// always-present inspector target — `/context`'s mouse route).
#[must_use]
pub fn context_meter_hitbox(info: &InfoLine<'_>, area: Rect) -> Option<Rect> {
    infoline_hitboxes(info, area)
        .into_iter()
        .find(|hitbox| hitbox.id == InfoSegmentId::Context)
        .map(|hitbox| hitbox.area)
}

impl Widget for InfoLine<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 || area.width < 1 {
            return;
        }
        let theme = self.theme;
        let ascii = self.ascii_safe;
        let ShedRow { kept, show_help } = shed_pass(&self, area);

        let right_edge = usize::from(area.x) + usize::from(area.width);
        let mut x = usize::from(area.x);
        let y = area.y;
        let join = sym(ITEM_JOIN, ascii);
        // Every write clips at the row's right edge, so a row below the
        // floor (model + context) truncates rather than wraps or panics.
        let set = |buf: &mut Buffer, cx: usize, span: &Span<'_>| {
            let budget = right_edge.saturating_sub(cx);
            if budget > 0 {
                buf.set_span(cx as u16, y, span, budget as u16);
            }
        };

        for (index, segment) in kept.iter().enumerate() {
            if index > 0 {
                set(
                    buf,
                    x,
                    &Span::styled(&join, chrome(theme, ChromeInk::MetadataDim)),
                );
                x += join.width();
            }
            // Slice G global rule: every actionable segment brightens on
            // hover. Model and Context own click actions; status-only
            // facts never do.
            let hovered = matches!(segment.id, InfoSegmentId::Model | InfoSegmentId::Context)
                && self.hovered == Some(segment.id);
            let mut style = chrome(theme, segment.ink);
            if hovered {
                style = style
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED);
            }
            // label dim, value in the segment's ink (two spans, one hitbox).
            // The label may be a glyph (`↓`); ascii-safe projects it, and
            // every projection is single-width so the shed arithmetic above
            // stays exact.
            if !segment.label.is_empty() {
                // A reading that has become a problem reads as one warning,
                // not a gray word beside a red number.
                let label_ink = match segment.ink {
                    ChromeInk::Failure | ChromeInk::Attention => segment.ink,
                    _ => ChromeInk::Metadata,
                };
                let label = sym(&segment.label, ascii);
                set(
                    buf,
                    x,
                    &Span::styled(label.clone(), chrome(theme, label_ink)),
                );
                x += label.width() + 1;
            }
            set(buf, x, &Span::styled(&segment.value, style));
            x += segment.value.width();
        }

        // The help hint is pinned to the row's right edge.
        if show_help {
            let hint = sym(self.help_hint, ascii);
            let sx = right_edge.saturating_sub(hint.width());
            set(
                buf,
                sx,
                &Span::styled(hint, chrome(theme, ChromeInk::MetadataHint)),
            );
        }
    }
}

fn chrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    crate::palette::grammar::chrome_style(theme, ink)
}

/// Recorded hitboxes for one rendered row. Mirrors the
/// `viewport.last_workflow_cancel_area` storage pattern: render computes the
/// rects, the caller stores them, `mouse_ui` hit-tests against them.
#[derive(Debug, Clone)]
pub struct InfoLineHitbox {
    pub id: InfoSegmentId,
    pub area: Rect,
}

/// Compute the hitbox `Rect` for each kept segment. Must be called with the
/// same inputs as the render so the rects match the painted cells exactly.
#[must_use]
pub fn infoline_hitboxes(info: &InfoLine<'_>, area: Rect) -> Vec<InfoLineHitbox> {
    let mut out = Vec::new();
    if area.height < 1 || area.width < 1 {
        return out;
    }
    let shed = shed_pass(info, area);
    let clip_right = usize::from(area.x) + usize::from(area.width);
    let join_width = sym(ITEM_JOIN, info.ascii_safe).width();
    let mut x = usize::from(area.x);
    for (index, segment) in shed.kept.iter().enumerate() {
        if index > 0 {
            x += join_width;
        }
        let w = segment.rendered_width(info.ascii_safe);
        let end = (x + w).min(clip_right);
        if x < end {
            out.push(InfoLineHitbox {
                id: segment.id,
                area: Rect {
                    x: x as u16,
                    y: area.y,
                    width: (end - x) as u16,
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
