//! The shell's info line — one row of session facts, painted under the
//! composer's posture row at the bottom of the screen.
//!
//! It used to be a top bar. The founder's call (SHELL-DESIGN-20260901 §2.0):
//! *"Putting the info at the bottom is a better idea, because then you scroll
//! up and it feels intentional. Move the top/side bar to the bottom."* The
//! segments, the shed pass, and the repository probe are the same code; only
//! the placement changed, and nothing paints above the transcript any more.
//!
//! The row, left to right, joined by ` · `:
//!
//! ```text
//! Hmbown/CodeWhale · ⑂ main · deepseek-v4 · context 61% ▰▰▰▰▰▰▱▱▱▱  Ctrl+/ help
//! ```
//!
//! The `codewhale` wordmark is gone from here: the mark belongs to the launch
//! header and nowhere else in the default look (§2.0 decision 4). So is the
//! `model` label — a bare model name in this row is self-describing. There is
//! no clock; a date stamp is not a fact anyone runs an agent to read, and it
//! used to outrank `model not connected` on the row it shared.
//!
//! The context reading is painted here and only here — the merged footer
//! above used to print the same percentage a second time from the same
//! snapshot.
//!
//! Shed order as width drops (spec §5b): the meter's bar glyphs first, then
//! the help hint, then any segment that carries a shorter form takes it (the
//! repository segment's `owner/name` becomes the folder basename), then
//! contextual segments by [`InfoSegmentId::shed_priority`] (folder, branch,
//! then the work facts). The repository outranks the hint on purpose: which
//! repository you are in is the fact people scan this row for, and the hint
//! comes back as soon as the row can afford both. The route identity and the
//! `context NN%` text are the floor and never shed; below the floor the
//! reading pins to the right edge whole and the route's tail is covered.
//!
//! Interaction: segment geometry is recorded for parity tests, but only the
//! effective model/route segment and the context meter advertise an action in
//! the live shell. Status-only facts do not brighten on hover or pretend to
//! be controls.
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
/// Width of the context meter bar (cells of ▰/▱).
const METER_CELLS: usize = 10;

/// Identity of an info-line segment. Most variants are status facts; the live
/// shell currently registers an action only for [`Self::Model`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InfoSegmentId {
    /// The repository the session is in: the `owner/name` forge slug when
    /// one is known, else the workspace folder name.
    Workspace,
    /// Checked-out git branch (`⑂ main`), from the cached status probe.
    Branch,
    /// Current run (work screen).
    Run,
    /// Active pod (work screen).
    Pod,
    /// Whale capacity `n/m` (work screen).
    Whales,
    /// Scheduled automation work `⏱ N scheduled · M running` — the
    /// `AutomationPanelState` projection owns the count; this row reads it.
    Automation,
    /// Effective model / route — click opens the provider inspector.
    Model,
    /// Settings breadcrumb (settings screen) — click walks up one category.
    /// Not constructed by the main shell yet: the settings screen is a
    /// later Tideline slice (spec §5a).
    #[allow(dead_code)]
    SettingsPath,
}

impl InfoSegmentId {
    /// Shed priority: higher sheds first as width drops. `0` never sheds.
    /// Segments only start shedding after the meter bar, the help hint, and
    /// every available shorter form have already gone. The floor is the
    /// route identity plus the `context NN%` text; among segments the
    /// declared order is folder, then branch, then the work facts, because
    /// route identity is the one fact the user must always be able to read
    /// (spec §5b).
    #[must_use]
    pub fn shed_priority(self) -> u8 {
        match self {
            Self::Workspace => 5,
            Self::Branch => 4,
            Self::Whales | Self::Automation => 3,
            Self::Pod => 2,
            Self::Run | Self::SettingsPath => 1,
            Self::Model => 0,
        }
    }
}

/// One contextual info-line segment.
#[derive(Debug, Clone)]
pub struct InfoSegment {
    pub id: InfoSegmentId,
    pub label: String,
    pub value: String,
    /// A shorter, still-true form of [`Self::value`]. The shed pass takes it
    /// before it drops anything, so a long value costs the row nothing it
    /// cannot get back. `None` means this segment has only one form.
    pub short: Option<String>,
    pub ink: ChromeInk,
}

impl InfoSegment {
    #[must_use]
    pub fn new(id: InfoSegmentId, label: &str, value: impl Into<String>, ink: ChromeInk) -> Self {
        Self {
            id,
            label: label.to_string(),
            value: value.into(),
            short: None,
            ink,
        }
    }

    /// Give the segment a shorter alternative form (ignored when it is not
    /// actually shorter — a "short" form that costs cells is not one).
    #[must_use]
    pub fn short(mut self, short: impl Into<String>) -> Self {
        let short = short.into();
        if short.width() < self.value.width() {
            self.short = Some(short);
        }
        self
    }

    fn value_at(&self, short: bool) -> &str {
        match (short, self.short.as_deref()) {
            (true, Some(short)) => short,
            _ => &self.value,
        }
    }

    fn rendered_width(&self, short: bool) -> usize {
        segment_text(self, short).width()
    }
}

fn segment_text(segment: &InfoSegment, short: bool) -> String {
    let value = segment.value_at(short);
    if segment.label.is_empty() {
        value.to_string()
    } else {
        format!("{} {}", segment.label, value)
    }
}

/// What the caller owes the info line. Everything is injected so renders are
/// deterministic (golden buffers) and wall-clock keyed by the owner, never
/// frame-count keyed (spec §5e).
pub struct InfoLine<'a> {
    pub theme: &'a UiTheme,
    /// The single right-hand key hint, e.g. `Ctrl+/ help`. Empty means the
    /// caller has no hint to advertise. It sheds right after the meter bar.
    pub help_hint: &'a str,
    pub context_label: &'a str,
    /// Context window percentage, 0–100.
    pub context_percent: u8,
    /// Contextual segments in display order.
    pub segments: &'a [InfoSegment],
    /// Actionable segment under the mouse. Only [`InfoSegmentId::Model`]
    /// currently advertises hover feedback in the live shell.
    pub hovered: Option<InfoSegmentId>,
    /// ASCII-safe / NO_COLOR mode: every glyph goes through
    /// [`glyphs::ascii_fallback`].
    pub ascii_safe: bool,
}

impl<'a> InfoLine<'a> {
    #[must_use]
    pub fn new(
        theme: &'a UiTheme,
        help_hint: &'a str,
        context_label: &'a str,
        context_percent: u8,
        segments: &'a [InfoSegment],
    ) -> Self {
        Self {
            theme,
            help_hint,
            context_label,
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

/// Ink for the meter bar and the percentage. At the 80% cap the whole
/// context reading turns to the error token — it is the one fact on this row
/// that becomes a problem rather than a status.
fn meter_ink_for(pct: u8) -> ChromeInk {
    if pct >= 80 {
        ChromeInk::Failure
    } else {
        ChromeInk::Info
    }
}

/// Ink for the `context ` label. Follows the value into the error token at
/// the cap so the reading reads as one warning, not a gray word beside a
/// red number.
fn context_label_ink_for(pct: u8) -> ChromeInk {
    if pct >= 80 {
        ChromeInk::Failure
    } else {
        ChromeInk::Metadata
    }
}

/// The context reading's text at one shed state — the row's last left item,
/// used for width arithmetic and mirrored span-for-span by the render.
fn context_text(label: &str, pct: u8, meter: &str, show_bar: bool) -> String {
    if show_bar {
        format!("{label} {pct}% {meter}")
    } else {
        format!("{label} {pct}%")
    }
}

/// The shed pass's answer: which segments survive at this row width, whether
/// the help hint and meter bar survived, and where the context reading sits.
/// Shared by the render and the hitbox computation so the two can never
/// disagree about the cells the meter painted — the same single-arithmetic
/// discipline the startup stage's `startup_layout` follows.
struct ShedRow<'t> {
    kept: Vec<&'t InfoSegment>,
    /// Segments with a shorter form are painting it.
    use_short: bool,
    show_bar: bool,
    show_help: bool,
    /// Width of the left run: segments, joins, and the context reading.
    left_width: usize,
    /// Width of the `context NN% ▰▰▱▱▱` item alone.
    context_width: usize,
    meter: String,
}

fn shed_pass<'t>(info: &'t InfoLine<'_>, area: Rect) -> ShedRow<'t> {
    let ascii = info.ascii_safe;
    let pct = info.context_percent.clamp(0, 100);
    let meter: String = (0..METER_CELLS)
        .map(|i| {
            let filled = (i + 1) * 100 / METER_CELLS <= usize::from(pct);
            sym(if filled { "▰" } else { "▱" }, ascii)
        })
        .collect();
    let help = sym(info.help_hint, ascii);

    let join_w = sym(ITEM_JOIN, ascii).width();
    let mut kept: Vec<&InfoSegment> = info.segments.iter().collect();
    // The left run is the kept segments plus the context reading, all joined.
    let left_width = |segs: &[&InfoSegment], short: bool, show_bar: bool| -> usize {
        segs.iter().map(|s| s.rendered_width(short)).sum::<usize>()
            + join_w * segs.len()
            + context_text(info.context_label, pct, &meter, show_bar).width()
    };
    let total_needed = |left: usize, show_help: bool| -> usize {
        left + if show_help && !help.is_empty() {
            HELP_GAP + help.width()
        } else {
            0
        }
    };

    // Shed pass, in the declared order: the meter's bar glyphs, then the help
    // hint, then any shorter segment form, then segments by priority. The bar
    // goes first on purpose — it encodes the same number printed beside it,
    // so it is the cheapest thing on the row to lose, and no repository,
    // branch, or model name should be cut to keep ten decorative cells.
    let mut show_help = !help.is_empty();
    let mut show_bar = true;
    let mut use_short = false;
    while total_needed(left_width(&kept, use_short, show_bar), show_help) > area.width as usize {
        if show_bar {
            show_bar = false;
        } else if show_help {
            show_help = false;
        } else if !use_short && kept.iter().any(|segment| segment.short.is_some()) {
            // A long `owner/name` degrades to the folder basename rather than
            // costing the row a whole segment — but only after the hint has
            // gone, because "which repository am I in" is what this row is
            // read for.
            use_short = true;
        } else if let Some(pos) = kept
            .iter()
            .enumerate()
            .filter(|(_, s)| s.id.shed_priority() > 0)
            .max_by_key(|(_, s)| s.id.shed_priority())
            .map(|(i, _)| i)
        {
            kept.remove(pos);
        } else {
            break;
        }
    }

    ShedRow {
        left_width: left_width(&kept, use_short, show_bar),
        context_width: context_text(info.context_label, pct, &meter, show_bar).width(),
        kept,
        use_short,
        show_bar,
        show_help,
        meter,
    }
}

/// The context meter's hitbox (spec §6: the meter is the chrome row's one
/// always-present inspector target — `Alt+C`'s mouse route). Covers the
/// painted context reading, pinned to the right when the row is below its
/// width floor.
#[must_use]
pub fn context_meter_hitbox(info: &InfoLine<'_>, area: Rect) -> Option<Rect> {
    if area.width < 1 || area.height < 1 {
        return None;
    }
    let shed = shed_pass(info, area);
    let area_left = usize::from(area.x);
    let area_right = area_left + usize::from(area.width);
    let start = if shed.left_width > usize::from(area.width) {
        area_right.saturating_sub(shed.context_width)
    } else {
        area_left + shed.left_width - shed.context_width
    };
    let end = (start + shed.context_width).min(area_right);
    if start >= end {
        return None;
    }
    Some(Rect {
        x: u16::try_from(start).unwrap_or(u16::MAX),
        y: area.y,
        width: u16::try_from(end - start).unwrap_or(area.width),
        height: 1,
    })
}

impl Widget for InfoLine<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 || area.width < 1 {
            return;
        }
        let theme = self.theme;
        let ascii = self.ascii_safe;
        let pct = self.context_percent.clamp(0, 100);
        let meter_ink = meter_ink_for(pct);
        let label_ink = context_label_ink_for(pct);
        let ShedRow {
            kept,
            use_short,
            show_bar,
            show_help,
            left_width,
            context_width,
            meter,
        } = shed_pass(&self, area);

        let mut x = area.x as usize;
        let y = area.y;
        let join = sym(ITEM_JOIN, ascii);
        // All positions below are usize and cast at the `set_span` boundary;
        // every write is clamped inside `area` by construction.
        let set = |buf: &mut Buffer, cx: usize, span: &Span<'_>| {
            buf.set_span(cx as u16, y, span, span.content.width() as u16);
        };
        let paint_join = |buf: &mut Buffer, cx: usize| {
            set(
                buf,
                cx,
                &Span::styled(&join, chrome(theme, ChromeInk::MetadataDim)),
            );
        };

        for (index, segment) in kept.iter().enumerate() {
            if index > 0 {
                paint_join(buf, x);
                x += join.width();
            }
            let hovered =
                segment.id == InfoSegmentId::Model && self.hovered == Some(InfoSegmentId::Model);
            let mut style = chrome(theme, segment.ink);
            if hovered {
                style = style
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED);
            }
            // label dim, value in the segment's ink (two spans, one hitbox)
            let value = segment.value_at(use_short);
            if segment.label.is_empty() {
                set(buf, x, &Span::styled(value, style));
                x += value.width();
            } else {
                // The label may be a glyph (`⑂`); ascii-safe projects it, and
                // every projection is single-width so the shed arithmetic
                // above stays exact.
                let label = sym(&segment.label, ascii);
                set(
                    buf,
                    x,
                    &Span::styled(label.clone(), chrome(theme, ChromeInk::Metadata)),
                );
                x += label.width() + 1;
                set(buf, x, &Span::styled(value, style));
                x += value.width();
            }
        }

        // The context reading closes the left run. Below the floor — a row
        // too narrow for even the route identity and the reading — the join
        // is skipped and the reading pins to the right edge instead, so the
        // number is whole and the route's tail is what gets covered: the
        // reading is the one fact that must always be readable, and a route
        // name cut short is still recognisable.
        let below_floor = left_width > usize::from(area.width);
        if below_floor {
            x = (area.x as usize + area.width as usize).saturating_sub(context_width);
        } else if !kept.is_empty() {
            paint_join(buf, x);
            x += join.width();
        }
        let context_prefix = format!("{} ", self.context_label);
        set(
            buf,
            x,
            &Span::styled(&context_prefix, chrome(theme, label_ink)),
        );
        x += context_prefix.width();
        let pct_text = format!("{pct}%");
        set(buf, x, &Span::styled(&pct_text, chrome(theme, meter_ink)));
        x += pct_text.width();
        if show_bar {
            set(buf, x, &Span::raw(" "));
            set(buf, x + 1, &Span::styled(&meter, chrome(theme, meter_ink)));
        }

        // The help hint is pinned to the row's right edge.
        if show_help {
            let hint = sym(self.help_hint, ascii);
            let sx = (area.x as usize + area.width as usize).saturating_sub(hint.width());
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

/// Recorded hitboxes for one rendered info row. Mirrors the
/// `viewport.last_workflow_cancel_area` storage pattern: render computes the
/// rects, the caller stores them, `mouse_ui` hit-tests against them.
#[derive(Debug, Clone)]
pub struct InfoLineHitbox {
    pub id: InfoSegmentId,
    pub area: Rect,
}

/// Compute the hitbox `Rect` for each kept segment. Must be called with the
/// same inputs as the render so the rects match the painted cells exactly.
/// The context meter has its own exact hitbox helper.
#[must_use]
pub fn infoline_hitboxes(info: &InfoLine<'_>, area: Rect) -> Vec<InfoLineHitbox> {
    let mut out = Vec::new();
    if area.height < 1 || area.width < 1 {
        return out;
    }
    let shed = shed_pass(info, area);
    let area_right = usize::from(area.x) + usize::from(area.width);
    let clip_right = if shed.left_width > usize::from(area.width) {
        area_right.saturating_sub(shed.context_width)
    } else {
        area_right
    };
    let join_width = sym(ITEM_JOIN, info.ascii_safe).width();
    let mut x = area.x as usize;
    for (index, segment) in shed.kept.iter().enumerate() {
        if index > 0 {
            x += join_width;
        }
        let w = segment.rendered_width(shed.use_short);
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
