//! Ambient ocean life for the underwater transcript field.
//!
//! One clear owner for the fish school, jellyfish, bubbles, and the rare
//! whale cameo — nothing else lives in the water (2026-07-23 product
//! decision: seaweed and bio-dust are gone). Motion stays inside the
//! existing delta/interpolation path: this module never requests frames on
//! its own.
//!
//! Motion language (shared with the rest of the shell): every mark can lerp
//! between the water and its ink at a time-varying brightness. Fish carry a
//! travelling sin² wave, jellyfish a slow floor-bounded pulse, bubbles an
//! occasional raised-cosine glint. Phases are wall-clock keyed and entity
//! periods deliberately never match, so nothing strobes in sync.
//!
//! Fish swim on a wrap-around path: they exit one edge and re-enter the
//! other still facing their travel direction, so facing always equals
//! velocity by construction. Direction may only change while the school is
//! fully off-screen.
//!
//! Under reduced motion, entities remain visible but static.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::ocean::{self, OceanColumn};

/// Depth layers for parallax. Nearer life is larger, faster, and more visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Depth {
    Background,
    Midground,
    Foreground,
}

impl Depth {
    #[must_use]
    fn ink_index(self) -> usize {
        match self {
            Self::Background => 1,
            Self::Midground | Self::Foreground => 0,
        }
    }
}

/// Creature density tier mirrored from shell width/height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifeDensity {
    Sparse,
    Normal,
    Rich,
}

impl LifeDensity {
    #[must_use]
    pub fn from_area(area: Rect) -> Self {
        if area.width < 56 || area.height < 12 {
            Self::Sparse
        } else if area.width < 88 || area.height < 20 {
            Self::Normal
        } else {
            Self::Rich
        }
    }

    #[must_use]
    fn school_size(self) -> usize {
        // One loose wedge of real fish; two schools compete with the whale.
        match self {
            Self::Sparse => 3,
            Self::Normal => 5,
            Self::Rich => 7,
        }
    }

    #[must_use]
    fn jellyfish_count(self) -> usize {
        match self {
            Self::Sparse => 1,
            Self::Normal => 2,
            Self::Rich => 2,
        }
    }

    #[must_use]
    fn bubble_streams(self) -> usize {
        match self {
            Self::Sparse => 1,
            Self::Normal => 2,
            Self::Rich => 2,
        }
    }
}

/// Lower floors so smaller windows still retain some life (was 68×15).
/// Keep in sync with [`crate::tui::ocean::AMBIENT_MIN_WIDTH`].
pub const AMBIENT_MIN_WIDTH: u16 = crate::tui::ocean::AMBIENT_MIN_WIDTH;
pub const AMBIENT_MIN_HEIGHT: u16 = crate::tui::ocean::AMBIENT_MIN_HEIGHT;

/// Whale cameo state: brief breach → spout → fluke → submerge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhaleCameoPhase {
    Hidden,
    Breach,
    Spout,
    Fluke,
    Submerge,
}

/// Snapshot of ambient positions for one frame (memoized once per draw).
#[derive(Debug, Clone)]
struct FrameMarks {
    marks: Vec<AmbientMark>,
}

#[derive(Debug, Clone, Copy)]
struct AmbientMark {
    x: u16,
    y: u16,
    glyph: &'static str,
    depth: Depth,
    style_mod: Option<Modifier>,
    /// Time-varying glow in `[0, 1]`: the mark's ink is lerped from the
    /// painted water toward full ink at this amount. `None` renders the
    /// plain ink (legacy behavior for the whale cameo).
    brightness: Option<f32>,
}

/// Optional pointer reaction for fish dart / bubble rise.
#[derive(Debug, Clone, Copy, Default)]
pub struct AmbientCursor {
    pub column: u16,
    pub row: u16,
    /// When set, fish flee from this point for ~800 ms of shared ocean clock.
    pub flee_elapsed_ms: Option<u128>,
}

/// Optional whale cameo trigger (e.g. successful turn completion).
#[derive(Debug, Clone, Copy, Default)]
pub struct WhaleCameo {
    pub elapsed_ms: Option<u128>,
    /// Anchor column within the field (composer / center).
    pub anchor_x: u16,
    pub anchor_y: u16,
}

const WHALE_CAMEO_MS: u128 = 2_400;

/// Render ambient life into empty water cells of `area`.
#[allow(clippy::too_many_arguments)]
pub fn render_ambient_life(
    area: Rect,
    buf: &mut Buffer,
    inks: (Color, Color),
    lines: &[Line<'static>],
    elapsed_ms: u128,
    animated: bool,
    cursor: AmbientCursor,
    whale: WhaleCameo,
) {
    if area.width < AMBIENT_MIN_WIDTH || area.height < AMBIENT_MIN_HEIGHT {
        return;
    }

    let density = LifeDensity::from_area(area);
    let frame = build_frame_marks(area, elapsed_ms, animated, density, cursor, whale);
    paint_marks(area, buf, inks, lines, &frame);
}

fn build_frame_marks(
    area: Rect,
    elapsed_ms: u128,
    animated: bool,
    density: LifeDensity,
    cursor: AmbientCursor,
    whale: WhaleCameo,
) -> FrameMarks {
    let mut marks = Vec::with_capacity(48);
    let t = if animated { elapsed_ms } else { 0 };

    // Leave the empty-state brand band (center third) mostly clear so life
    // frames the room instead of littering the hero whale + status lines.
    let quiet_top = (area.height / 5).max(2);
    let quiet_mid_lo = area.height.saturating_mul(2) / 5;
    let quiet_mid_hi = area.height.saturating_mul(3) / 5;

    // --- One loose fish school on a wrap-around path ---
    // The school enters one edge, crosses, and exits the other; direction
    // may only change while it is fully off-screen, so facing always equals
    // velocity. A travelling sin² brightness wave runs through the wedge.
    let school_size = density.school_size().min(SCHOOL_WEDGE.len());
    let school_span = SCHOOL_WEDGE
        .iter()
        .take(school_size)
        .map(|(_, dx)| *dx)
        .max()
        .unwrap_or(0)
        .saturating_add(LEAD_FISH_RIGHT.len() as u16);
    let travel = u128::from(area.width.saturating_add(school_span).max(1));
    let cycle_ms = travel.saturating_mul(SCHOOL_CELL_MS);
    // Half-cycle head start: freshly opened water shows the school
    // mid-crossing instead of an empty entry beat.
    let school_clock = t.saturating_add(cycle_ms / 2);
    let (cycle_index, cycle_step) = if animated {
        (
            school_clock / cycle_ms,
            ((school_clock % cycle_ms) / SCHOOL_CELL_MS) as i32,
        )
    } else {
        // Reduced motion: park the school mid-crossing, facing right.
        (0, (travel / 2) as i32)
    };
    let swims_right = !animated || school_swims_right(cycle_index);
    // Alternate the travel band between crossings; both avoid the hero band.
    let swims_low = school_swims_low(cycle_index);
    let anchor_y = if swims_low {
        area.height.saturating_mul(3) / 4
    } else {
        quiet_top.saturating_add(1)
    };
    let ptr = cursor.column.saturating_sub(area.x);
    let ptr_y = cursor.row.saturating_sub(area.y);
    for (m, (dy, dx)) in SCHOOL_WEDGE.iter().take(school_size).enumerate() {
        let body = fish_body(swims_right, m == 0);
        let body_w = body.len() as u16; // ASCII bodies: len == width
        // Nose position in wrap space; trailers sit `dx` columns behind the
        // lead relative to travel, so the wedge follows instead of leading.
        // Right-swimmers enter from the left edge, left-swimmers from the
        // right edge — both facing exactly the way they move.
        let mut x_i32 = if swims_right {
            cycle_step - 1 - i32::from(*dx) - (i32::from(body_w) - 1)
        } else {
            i32::from(area.width) - cycle_step + i32::from(*dx)
        };
        // Slight per-fish vertical stagger + slow bob.
        let bob = if animated {
            sine_bob(t, 3_400 + (m as u128) * 640, 1)
        } else {
            0
        };
        let mut y_i32 = i32::from(anchor_y) + i32::from(*dy) + i32::from(bob);
        // Fish flee the pointer in both dimensions (nearby motion only).
        if let Some(flee_ms) = cursor.flee_elapsed_ms {
            let flee = i32::from(fish_flee_offset(flee_ms));
            if x_i32.abs_diff(i32::from(ptr)) < 16 && y_i32.abs_diff(i32::from(ptr_y)) < 6 {
                if x_i32 >= i32::from(ptr) {
                    x_i32 += flee;
                } else {
                    x_i32 -= flee;
                }
                if y_i32 >= i32::from(ptr_y) {
                    y_i32 += 1;
                } else {
                    y_i32 -= 1;
                }
            }
        }
        let max_x = i32::from(area.width.saturating_sub(body_w));
        let max_y = i32::from(area.height.saturating_sub(1));
        if x_i32 < 0 || x_i32 > max_x || y_i32 < 0 || y_i32 > max_y {
            continue; // off-screen while wrapping
        }
        let y = y_i32 as u16;
        // Never swim through the hero band.
        if y > quiet_mid_lo && y < quiet_mid_hi {
            continue;
        }
        let brightness = if animated {
            FISH_BRIGHTNESS_FLOOR
                + (1.0 - FISH_BRIGHTNESS_FLOOR)
                    * wave01(t, FISH_WAVE_MS, (m as u128).saturating_mul(320))
        } else {
            0.7
        };
        marks.push(AmbientMark {
            x: x_i32 as u16,
            y,
            glyph: body,
            depth: if m == 0 {
                Depth::Foreground
            } else {
                Depth::Midground
            },
            style_mod: None,
            brightness: Some(brightness),
        });
    }

    // --- Jellyfish: a bell with a lagging tentacle pulse ---
    // The bell pulses on a slow floor-bounded sin²; the tentacle repeats the
    // pulse ~350 ms later — the lag is what sells "jellyfish". They drift
    // slowly upward through the side lanes and wrap.
    for j in 0..density.jellyfish_count() {
        let phase = 3_100u128.saturating_add((j as u128) * 4_700);
        let lane_x = if j % 2 == 0 {
            area.width.saturating_mul(5) / 6
        } else {
            area.width / 6
        };
        let wobble = if animated {
            sine_bob(t, 5_200 + phase, 1)
        } else {
            0
        };
        let x = lane_x
            .saturating_add(wobble)
            .min(area.width.saturating_sub(JELLY_BELL.len() as u16 + 1));
        let rise_rows = u128::from(area.height.saturating_sub(4).max(1));
        let rise_period = 8_600u128.saturating_add((j as u128) * 1_400);
        let risen = if animated {
            ((t.saturating_add(phase) / rise_period) % rise_rows) as u16
        } else {
            (rise_rows / 2) as u16
        };
        let y = area.height.saturating_sub(2).saturating_sub(risen);
        if y == 0 || (y > quiet_mid_lo && y < quiet_mid_hi) {
            continue;
        }
        let bell_brightness = if animated {
            JELLY_BRIGHTNESS_FLOOR
                + (1.0 - JELLY_BRIGHTNESS_FLOOR) * wave01(t, JELLY_PULSE_MS, phase)
        } else {
            0.6
        };
        let tentacle_brightness = if animated {
            JELLY_BRIGHTNESS_FLOOR
                + (1.0 - JELLY_BRIGHTNESS_FLOOR)
                    * wave01(
                        t.saturating_sub(JELLY_TENTACLE_LAG_MS),
                        JELLY_PULSE_MS,
                        phase,
                    )
        } else {
            0.45
        };
        marks.push(AmbientMark {
            x,
            y,
            glyph: JELLY_BELL,
            depth: Depth::Midground,
            style_mod: None,
            brightness: Some(bell_brightness),
        });
        if y + 1 < area.height && !(y + 1 > quiet_mid_lo && y + 1 < quiet_mid_hi) {
            let sway = if animated {
                JELLY_TENTACLE_FRAMES
                    [((t.saturating_add(phase) / 1_400) as usize) % JELLY_TENTACLE_FRAMES.len()]
            } else {
                "|"
            };
            marks.push(AmbientMark {
                x: x.saturating_add(1),
                y: y + 1,
                glyph: sway,
                depth: Depth::Background,
                style_mod: None,
                brightness: Some(tentacle_brightness),
            });
        }
    }

    // --- Rising bubble streams (quiet ·/˚ with occasional glints) ---
    for b in 0..density.bubble_streams() {
        let phase = (b as u128).saturating_mul(1_900);
        // Edge columns — avoid center brand.
        let column = if b % 2 == 0 {
            area.width / 8
        } else {
            area.width.saturating_mul(7) / 8
        };
        let rise_period = 3_200u128.saturating_add(phase % 900);
        let rise = if animated {
            let cycle = (t.saturating_add(phase) % rise_period) as f64 / rise_period as f64;
            let max_rise = area.height.saturating_sub(3) as f64;
            (cycle * max_rise) as u16
        } else {
            area.height / 4
        };
        let boost = if cursor.flee_elapsed_ms.is_some() && column.abs_diff(ptr) < 10 {
            2
        } else {
            0
        };
        let y = area
            .height
            .saturating_sub(2)
            .saturating_sub(rise.saturating_add(boost))
            .max(quiet_top);
        // Skip the empty-state text band.
        if y > quiet_mid_lo && y < quiet_mid_hi {
            continue;
        }
        let glyph = if animated {
            ["·", "˚", "·", "°"][((t.saturating_add(phase)) / 320) as usize % 4]
        } else {
            "·"
        };
        let brightness = if animated {
            glint01(t, 2_600 + phase % 700, 600, BUBBLE_BRIGHTNESS_FLOOR, phase)
        } else {
            BUBBLE_BRIGHTNESS_FLOOR
        };
        marks.push(AmbientMark {
            x: column.min(area.width.saturating_sub(1)),
            y,
            glyph,
            depth: Depth::Foreground,
            style_mod: None,
            brightness: Some(brightness),
        });
    }

    // --- Rare whale cameo (completion only) ---
    if let Some(cameo_ms) = whale.elapsed_ms.filter(|ms| *ms < WHALE_CAMEO_MS) {
        let phase = whale_cameo_phase(cameo_ms);
        if phase != WhaleCameoPhase::Hidden {
            let ax = whale
                .anchor_x
                .saturating_sub(area.x)
                .min(area.width.saturating_sub(4));
            let ay = whale
                .anchor_y
                .saturating_sub(area.y)
                .min(area.height.saturating_sub(2));
            let (glyph, y_off) = match phase {
                WhaleCameoPhase::Breach => ("≈≈>", 0u16),
                WhaleCameoPhase::Spout => ("≈≈>", 0),
                WhaleCameoPhase::Fluke => ("～", 1),
                WhaleCameoPhase::Submerge => ("·", 1),
                WhaleCameoPhase::Hidden => ("", 0),
            };
            if !glyph.is_empty() {
                marks.push(AmbientMark {
                    x: ax,
                    y: ay.saturating_add(y_off).min(area.height.saturating_sub(1)),
                    glyph,
                    depth: Depth::Foreground,
                    style_mod: None,
                    brightness: None,
                });
                if phase == WhaleCameoPhase::Spout && ay > 0 {
                    marks.push(AmbientMark {
                        x: ax.saturating_add(1).min(area.width.saturating_sub(1)),
                        y: ay.saturating_sub(1),
                        glyph: "˚",
                        depth: Depth::Foreground,
                        style_mod: Some(Modifier::DIM),
                        brightness: None,
                    });
                }
            }
        }
    }

    FrameMarks { marks }
}

/// Loose diagonal wedge for the school: `(row_offset, columns_behind_lead)`.
/// The slight row spread is what makes it read as a school, not a text row.
const SCHOOL_WEDGE: &[(i16, u16)] = &[(0, 0), (-1, 4), (1, 6), (-2, 9), (2, 11), (0, 14), (-1, 17)];

/// Wall-clock milliseconds per column of school travel (~2.6 cells/s).
const SCHOOL_CELL_MS: u128 = 380;
/// Travelling brightness-wave period through the wedge.
const FISH_WAVE_MS: u128 = 2_200;
/// Fish are small: never let one sink into the gradient.
const FISH_BRIGHTNESS_FLOOR: f32 = 0.45;

/// Lead fish silhouettes (ASCII only — width == len). Members drop the eye.
const LEAD_FISH_RIGHT: &str = "><o>";
const LEAD_FISH_LEFT: &str = "<o><";

/// Jellyfish bell and its swaying tentacle frames (all width-1 ASCII).
// Dome + lagging tentacle: read as jellyfish, not a parenthetical blob-on-a-string.
// Keep ASCII-adjacent glyphs that render cleanly in common terminal fonts.
const JELLY_BELL: &str = "o*";
// Dome + lagging tentacle: jellyfish silhouette in ASCII-safe glyphs so the
// low-motion / ascii_safe tier still covers ambient life.
const JELLY_TENTACLE_FRAMES: &[&str] = &["|", ":", "|", "."];

const JELLY_PULSE_MS: u128 = 2_900;
/// The tentacle repeats the bell pulse this much later.
const JELLY_TENTACLE_LAG_MS: u128 = 350;
const JELLY_BRIGHTNESS_FLOOR: f32 = 0.35;

/// Bubbles stay mostly steady with occasional glints, not a constant wave.
const BUBBLE_BRIGHTNESS_FLOOR: f32 = 0.55;

/// One soft sin² hump per `period_ms`, wall-clock keyed, in `[0, 1]`.
#[must_use]
fn wave01(elapsed_ms: u128, period_ms: u128, phase_ms: u128) -> f32 {
    if period_ms == 0 {
        return 1.0;
    }
    let frac = (elapsed_ms.saturating_add(phase_ms) % period_ms) as f64 / period_ms as f64;
    let s = (frac * std::f64::consts::PI).sin();
    (s * s) as f32
}

/// Mostly `floor`, with a raised-cosine glint to full brightness for
/// `glint_ms` out of every `period_ms`.
#[must_use]
fn glint01(elapsed_ms: u128, period_ms: u128, glint_ms: u128, floor: f32, phase_ms: u128) -> f32 {
    if period_ms == 0 || glint_ms == 0 {
        return floor;
    }
    let pos = elapsed_ms.saturating_add(phase_ms) % period_ms;
    if pos >= glint_ms {
        return floor;
    }
    let frac = pos as f64 / glint_ms as f64;
    let bump = 0.5 * (1.0 - (frac * std::f64::consts::TAU).cos());
    floor + (1.0 - floor) * bump as f32
}

/// Stateless per-crossing travel direction. Direction only ever changes
/// between cycles — while the school is fully off-screen — so a turn is
/// never visible as an in-place flip.
#[must_use]
fn school_swims_right(cycle_index: u128) -> bool {
    (cycle_index.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 7) & 1 == 0
}

/// Stateless per-crossing band choice (lower vs upper third).
#[must_use]
fn school_swims_low(cycle_index: u128) -> bool {
    (cycle_index.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) >> 9) & 1 == 0
}

fn paint_marks(
    area: Rect,
    buf: &mut Buffer,
    inks: (Color, Color),
    lines: &[Line<'static>],
    frame: &FrameMarks,
) {
    for mark in &frame.marks {
        let protected = lines
            .get(usize::from(mark.y))
            .and_then(occupied_text_bounds);
        let mark_width = UnicodeWidthStr::width(mark.glyph);
        let collides = protected.is_some_and(|(start, end)| {
            usize::from(mark.x) < end.saturating_add(1)
                && usize::from(mark.x) + mark_width > start.saturating_sub(1)
        });
        if collides || mark.x.saturating_add(mark_width as u16) > area.width {
            continue;
        }
        let ink = if mark.depth.ink_index() == 1 {
            inks.1
        } else {
            inks.0
        };
        for (offset, ch) in mark.glyph.chars().enumerate() {
            let cell = &mut buf[(area.x + mark.x + offset as u16, area.y + mark.y)];
            // Glow language: lerp the mark's ink up from the water the cell
            // already sits in, at the entity's time-varying brightness.
            let fg = match (mark.brightness, cell.style().bg) {
                (Some(amount), Some(water)) => {
                    ocean::mix_colors(water, ink, amount.clamp(0.0, 1.0))
                }
                (Some(amount), None) => ocean::scale_color(ink, amount.clamp(0.0, 1.0).max(0.4)),
                (None, _) => ink,
            };
            let mut style = Style::default().fg(fg);
            if let Some(m) = mark.style_mod {
                style = style.add_modifier(m);
            }
            cell.set_symbol(&ch.to_string());
            cell.set_style(style);
        }
    }
}

/// Width-only occupied-text measurement (no per-line String allocation).
#[must_use]
pub fn occupied_text_bounds(line: &Line<'_>) -> Option<(usize, usize)> {
    if line.spans.is_empty() {
        return None;
    }
    let mut total = 0usize;
    let mut leading = 0usize;
    let mut seen_non_ws = false;
    let mut trailing_run = 0usize;

    for span in &line.spans {
        for ch in span.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            total = total.saturating_add(w);
            if ch.is_whitespace() {
                if !seen_non_ws {
                    leading = leading.saturating_add(w);
                } else {
                    trailing_run = trailing_run.saturating_add(w);
                }
            } else {
                seen_non_ws = true;
                trailing_run = 0;
            }
        }
    }
    if !seen_non_ws {
        return None;
    }
    Some((leading, total.saturating_sub(trailing_run)))
}

#[must_use]
fn sine_bob(elapsed_ms: u128, period_ms: u128, amplitude: u16) -> u16 {
    if period_ms == 0 || amplitude == 0 {
        return 0;
    }
    let phase = (elapsed_ms % period_ms) as f64 / period_ms as f64;
    let s = (phase * std::f64::consts::TAU).sin();
    // Map [-1,1] → [0, amplitude]
    (((s + 1.0) * 0.5) * f64::from(amplitude)).round() as u16
}

/// One-shot flee arc keyed to Working transition / pointer motion.
#[must_use]
pub fn fish_flee_offset(elapsed_ms: u128) -> u16 {
    let progress = elapsed_ms.min(800) as f32 / 800.0;
    let excursion = (progress * std::f32::consts::PI).sin() * 9.0;
    excursion.round().clamp(0.0, 9.0) as u16
}

/// One fish silhouette family for the whole school: the lead carries an eye
/// (`><o>`), members are plain `><>`. Never mix lone `>` arrows in — that
/// reads as broken punctuation. All bodies are ASCII so `len() == width`.
#[must_use]
fn fish_body(facing_right: bool, lead: bool) -> &'static str {
    match (facing_right, lead) {
        (true, true) => LEAD_FISH_RIGHT,
        (true, false) => "><>",
        (false, true) => LEAD_FISH_LEFT,
        (false, false) => "<><",
    }
}

#[must_use]
pub fn whale_cameo_phase(elapsed_ms: u128) -> WhaleCameoPhase {
    match elapsed_ms {
        0..400 => WhaleCameoPhase::Breach,
        400..1_000 => WhaleCameoPhase::Spout,
        1_000..1_700 => WhaleCameoPhase::Fluke,
        1_700..WHALE_CAMEO_MS => WhaleCameoPhase::Submerge,
        _ => WhaleCameoPhase::Hidden,
    }
}

/// Subtle caustic shimmer applied to empty water cells when the field would
/// otherwise read as a static ramp. Cheap: one phase lookup per cell, only
/// when `animated` and density allows.
pub fn apply_caustic_shimmer(
    area: Rect,
    buf: &mut Buffer,
    column: &OceanColumn,
    elapsed_ms: u128,
    animated: bool,
    lines: &[Line<'static>],
) {
    if !animated || area.width < AMBIENT_MIN_WIDTH || area.height < AMBIENT_MIN_HEIGHT {
        return;
    }
    // Sparse sampling: every 3rd column on every other row near the surface.
    let band = (area.height / 3).max(2);
    for local_y in 0..band {
        let protected = lines
            .get(usize::from(local_y))
            .and_then(occupied_text_bounds);
        let ramp = frame_ocean_ramp(
            column,
            area.height,
            area.y,
            elapsed_ms,
            column.phase_tag(),
            column.ramp_fingerprint(),
        );
        let row_bg = ramp
            .get(usize::from(local_y))
            .copied()
            .unwrap_or_else(|| column.color_at_y(area.y.saturating_add(local_y)));
        for local_x in (0..area.width).step_by(3) {
            if protected.is_some_and(|(start, end)| {
                usize::from(local_x) >= start && usize::from(local_x) < end
            }) {
                continue;
            }
            let phase = ((elapsed_ms / 80)
                .wrapping_add(u128::from(local_x))
                .wrapping_add(u128::from(local_y) * 3))
                % 12;
            if phase > 2 {
                continue;
            }
            let cell = &mut buf[(area.x + local_x, area.y + local_y)];
            // Soften toward ambient ink without replacing semantic glyphs.
            if cell.symbol() == " " || cell.symbol().is_empty() {
                let shimmer = ocean::scale_color(row_bg, 1.08);
                cell.set_bg(shimmer);
            }
        }
    }
}

/// Cached ocean row colors invalidated only when phase/dimensions/palette/breath tick.
/// Shared across widgets that paint the same [`OceanColumn`] within a frame.
#[derive(Debug, Clone, Default)]
pub struct OceanRampCache {
    colors: Vec<Color>,
    height: u16,
    top: u16,
    elapsed_bucket: u128,
    phase_tag: u8,
    ramp_fingerprint: u64,
}

impl OceanRampCache {
    /// Return a per-row color ramp, recomputing only when inputs change.
    pub fn colors_for(
        &mut self,
        column: &OceanColumn,
        height: u16,
        top: u16,
        elapsed_ms: u128,
        phase_tag: u8,
        ramp_fingerprint: u64,
    ) -> &[Color] {
        // Breath cycle is 90s; bucket at ~80ms atmosphere cadence so we don't
        // recompute every draw when nothing visible changed.
        let bucket = elapsed_ms / 80;
        if self.colors.len() == usize::from(height)
            && self.height == height
            && self.top == top
            && self.elapsed_bucket == bucket
            && self.phase_tag == phase_tag
            && self.ramp_fingerprint == ramp_fingerprint
        {
            return &self.colors;
        }
        self.colors.clear();
        self.colors.reserve(usize::from(height));
        for local_y in 0..height {
            self.colors
                .push(column.color_at_y(top.saturating_add(local_y)));
        }
        self.height = height;
        self.top = top;
        self.elapsed_bucket = bucket;
        self.phase_tag = phase_tag;
        self.ramp_fingerprint = ramp_fingerprint;
        &self.colors
    }
}

thread_local! {
    static FRAME_RAMP: std::cell::RefCell<OceanRampCache> =
        const { std::cell::RefCell::new(OceanRampCache {
            colors: Vec::new(),
            height: 0,
            top: 0,
            elapsed_bucket: 0,
            phase_tag: 0,
            ramp_fingerprint: 0,
        }) };
}

/// Process-local per-frame ocean ramp shared by chat field, caustics, and
/// other widgets that paint the same column.
#[must_use]
pub fn frame_ocean_ramp(
    column: &OceanColumn,
    height: u16,
    top: u16,
    elapsed_ms: u128,
    phase_tag: u8,
    ramp_fingerprint: u64,
) -> Vec<Color> {
    FRAME_RAMP.with(|cache| {
        cache
            .borrow_mut()
            .colors_for(column, height, top, elapsed_ms, phase_tag, ramp_fingerprint)
            .to_vec()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    #[test]
    fn ambient_min_dimensions_allow_small_windows() {
        assert!(AMBIENT_MIN_WIDTH < 68);
        assert!(AMBIENT_MIN_HEIGHT < 15);
    }

    #[test]
    fn occupied_text_bounds_skips_string_join() {
        let line = Line::from(vec![Span::raw("  hello  "), Span::raw("world  ")]);
        let (start, end) = occupied_text_bounds(&line).expect("bounds");
        assert_eq!(start, 2);
        assert!(end > start);
    }

    #[test]
    fn whale_cameo_is_brief() {
        assert_eq!(whale_cameo_phase(0), WhaleCameoPhase::Breach);
        assert_eq!(whale_cameo_phase(500), WhaleCameoPhase::Spout);
        assert_eq!(whale_cameo_phase(1_200), WhaleCameoPhase::Fluke);
        assert_eq!(whale_cameo_phase(2_000), WhaleCameoPhase::Submerge);
        assert_eq!(whale_cameo_phase(3_000), WhaleCameoPhase::Hidden);
    }

    #[test]
    fn density_scales_with_area() {
        assert_eq!(
            LifeDensity::from_area(Rect::new(0, 0, 40, 10)),
            LifeDensity::Sparse
        );
        assert_eq!(
            LifeDensity::from_area(Rect::new(0, 0, 100, 30)),
            LifeDensity::Rich
        );
    }

    #[test]
    fn fish_school_uses_one_silhouette_family() {
        // Never mix lone `>` with full fish bodies; the lead just gains an
        // eye within the same family.
        assert_eq!(fish_body(true, false), "><>");
        assert_eq!(fish_body(false, false), "<><");
        assert_eq!(fish_body(true, true), "><o>");
        assert_eq!(fish_body(false, true), "<o><");
    }

    fn frame_at(t: u128) -> FrameMarks {
        let area = Rect::new(0, 0, 100, 30);
        build_frame_marks(
            area,
            t,
            true,
            LifeDensity::from_area(area),
            AmbientCursor::default(),
            WhaleCameo::default(),
        )
    }

    #[test]
    fn fish_always_swim_the_way_they_face() {
        // Wrap-around construction: within one crossing, a right-facing
        // school only ever moves right, and a left-facing school only left.
        let area_travel = 100u128 + 21; // width + wedge span (see constants)
        let cycle_ms = area_travel * SCHOOL_CELL_MS;
        for cycle in 0u128..6 {
            // The school clock carries a half-cycle head start, so sampling
            // at cycle*cycle_ms lands mid-crossing of `cycle`.
            let t1 = cycle * cycle_ms;
            let t2 = t1 + SCHOOL_CELL_MS * 3;
            let lead = |t: u128| {
                frame_at(t)
                    .marks
                    .into_iter()
                    .find(|mark| mark.glyph.contains('o'))
            };
            let (Some(a), Some(b)) = (lead(t1), lead(t2)) else {
                continue; // school off-screen at this sample — fine
            };
            let expect_right = school_swims_right(cycle);
            if expect_right {
                assert_eq!(a.glyph, "><o>", "cycle {cycle} facing");
                assert!(b.x >= a.x, "cycle {cycle}: right-facing fish moved left");
            } else {
                assert_eq!(a.glyph, "<o><", "cycle {cycle} facing");
                assert!(b.x <= a.x, "cycle {cycle}: left-facing fish moved right");
            }
        }
        // Both directions must actually occur across nearby cycles.
        let dirs: Vec<bool> = (0u128..12).map(school_swims_right).collect();
        assert!(
            dirs.iter().any(|d| *d) && dirs.iter().any(|d| !*d),
            "{dirs:?}"
        );
    }

    #[test]
    fn water_holds_only_fish_bubbles_and_jellyfish() {
        // Seaweed and bio-dust are gone (2026-07-23): every mark is a fish
        // body, a bubble glyph, or a jellyfish part.
        for t in [0u128, 7_500, 33_000, 61_000, 120_000] {
            for mark in frame_at(t).marks {
                let ok = matches!(mark.glyph, "><>" | "<><" | "><o>" | "<o><")
                    || matches!(mark.glyph, "·" | "˚" | "°")
                    || mark.glyph == JELLY_BELL
                    || JELLY_TENTACLE_FRAMES.contains(&mark.glyph);
                assert!(ok, "unexpected ambient glyph {:?} at t={t}", mark.glyph);
            }
        }
    }

    #[test]
    fn jellyfish_reads_as_bell_with_lagging_tentacle() {
        // Find a frame where a jelly is on-screen and assert its two-row
        // structure: bell, tentacle one row below inside the bell, and the
        // tentacle pulse trailing the bell pulse.
        let mut seen = false;
        for probe in 0..240u128 {
            let t = probe * 500;
            let frame = frame_at(t);
            if let Some(bell) = frame.marks.iter().find(|mark| mark.glyph == JELLY_BELL) {
                if let Some(tentacle) = frame.marks.iter().find(|mark| {
                    JELLY_TENTACLE_FRAMES.contains(&mark.glyph)
                        && mark.y == bell.y + 1
                        && mark.x == bell.x + 1
                }) {
                    let bell_glow = bell.brightness.expect("bell pulses");
                    let tentacle_glow = tentacle.brightness.expect("tentacle pulses");
                    assert!(bell_glow >= JELLY_BRIGHTNESS_FLOOR - f32::EPSILON);
                    assert!(tentacle_glow >= JELLY_BRIGHTNESS_FLOOR - f32::EPSILON);
                    seen = true;
                    break;
                }
            }
        }
        assert!(seen, "no complete jellyfish found in 120s of frames");
    }

    #[test]
    fn glow_helpers_stay_bounded_with_floors() {
        for t in (0u128..12_000).step_by(97) {
            let w = wave01(t, FISH_WAVE_MS, 0);
            assert!((0.0..=1.0).contains(&w), "wave01 out of range: {w}");
            let g = glint01(t, 2_600, 600, BUBBLE_BRIGHTNESS_FLOOR, 0);
            assert!(
                (BUBBLE_BRIGHTNESS_FLOOR..=1.0).contains(&g),
                "glint01 lost its floor: {g}"
            );
        }
    }
}
