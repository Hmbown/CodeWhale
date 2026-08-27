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
//! travelling sin² wave, jellyfish a slow band-bounded pulse that opens and
//! closes the dome while the tentacles trail it by ~0.6 s, bubbles an
//! occasional raised-cosine glint. Phases are wall-clock keyed and entity
//! periods deliberately never match, so nothing strobes in sync.
//!
//! The jellyfish is a *visitor*, not scenery: one at most, present for roughly
//! a fifth of a ~5-minute cycle and dimmer than everything around it. See the
//! `JELLY_VISIT_*` constants for the rarity knobs and why they are set where
//! they are.
//!
//! Fish swim on a wrap-around path: they exit one edge and re-enter the
//! other still facing their travel direction, so facing always equals
//! velocity by construction. Direction may only change while the school is
//! fully off-screen.
//!
//! The aquarium has a habitat and it defers to whatever is composed above it.
//! Collision is one rule — [`is_open_water`]: a mark may only land in a
//! horizontal span that carries no text and has none within
//! [`TEXT_CLEARANCE_ROWS`] of it, measured off the rendered lines rather than
//! guessed from fractions of the field. Everything else follows from it. A
//! short status line therefore leaves honest water beside it instead of
//! claiming the whole row. The school rides a band off the
//! floor ([`SCHOOL_FLOOR_GAP`]); bubbles rise a few rows from the floor and
//! dissolve ([`BUBBLE_MAX_RISE_ROWS`]); the jellyfish only surfaces where
//! [`deep_water_rows`] says the water is deep enough to hold it *and* the
//! school; and the surface caustics stop at the first row of the composition.
//! Light above, life below, words in between — and as a transcript fills the
//! field the water closes row by row until nothing moves behind the text the
//! reader is actually reading.
//!
//! Two clocks feed this module and neither is a token counter. Positions ride
//! `App::sample_ambient_clock_ms`, which advances by real elapsed time clamped
//! to `App::AMBIENT_MAX_STEP_MS` per draw, so drift speed is identical at 16 ms
//! and 33 ms frames and a stalled-then-resumed frame cannot jump a creature.
//! Sideways *placement*, by contrast, is a function of the transcript text
//! under the silhouette — which does change with token throughput — so it is
//! bounded by [`JELLY_MAX_TEXT_DODGE_COLS`].
//!
//! Under reduced motion there is no ambient life at all: `ocean::life_presence`
//! returns 0 and [`paint_marks`] returns before writing a cell. Marks are still
//! *built* (the budget counters stay honest), just never painted.
//!
//! `render_ambient_life` returns per-frame budget counters
//! ([`AmbientFrameStats`]): marks built always splits exactly into painted +
//! text-skipped + clipped. Counting is a handful of `u32` increments — no
//! allocation, no frame requests.

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
        // At most one jellyfish in the water at a time, at every tier. Two
        // put a pulsing silhouette in *both* side lanes, which is what made
        // them read as resident scenery instead of a passing visitor. The
        // rarity knob that matters is the visit duty cycle
        // ([`JELLY_VISIT_CYCLE_SLOTS`]), not the population.
        match self {
            Self::Sparse | Self::Normal | Self::Rich => 1,
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
    /// Multi-row creature identity. Every part relocates or is withheld as one
    /// unit so a jellyfish never degrades into a detached dome or tentacles.
    jellyfish: Option<usize>,
    depth: Depth,
    style_mod: Option<Modifier>,
    /// Time-varying glow in `[0, 1]`: the mark's ink is lerped from the
    /// painted water toward full ink at this amount. `None` renders the
    /// plain ink (legacy behavior for the whale cameo).
    brightness: Option<f32>,
}

/// Per-frame render budget counters. `marks_built` splits exactly into
/// `marks_painted + marks_skipped_text + marks_clipped`. `cells_written`
/// counts individual cell writes: a multi-cell glyph counts each of its
/// cells, and two overlapping marks count the shared cell once per write.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AmbientFrameStats {
    pub marks_built: u32,
    pub marks_painted: u32,
    pub marks_skipped_text: u32,
    pub marks_clipped: u32,
    pub cells_written: u32,
}

/// Hard upper bound on marks built in one frame: 7 fish + 1 jellyfish × 4
/// parts (2 dome rows + 2 tentacles) + 2 bubbles + 2 whale-cameo cells = 15,
/// plus headroom. This is a test-gate ceiling asserted against
/// [`AmbientFrameStats::marks_built`], not a runtime clamp: the population is
/// bounded by construction, and this constant is what fails the build if a
/// future change makes it unbounded.
#[cfg(test)]
pub const MAX_FRAME_MARKS: u32 = 24;

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

/// How the ambient scene is shaped by live agent activity. The underwater
/// used to be phase-agnostic: same fish, same pace, whether the agent was
/// thinking, running tools, or orchestrating sub-agents. Each treatment is a
/// bounded parameter shift — never a second scene graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AmbientActivity {
    #[default]
    Baseline,
    Reasoning,
    /// Read-shaped exploration: quieter than generic tool work, brighter
    /// than hidden reasoning — skimming, not digging.
    Reading,
    Tools,
    Subagents,
    Verifying,
}

impl AmbientActivity {
    #[must_use]
    pub fn from_kind(kind: crate::tui::underwater::LiveActivityKind) -> Self {
        match kind {
            crate::tui::underwater::LiveActivityKind::Reasoning => Self::Reasoning,
            crate::tui::underwater::LiveActivityKind::Reading => Self::Reading,
            crate::tui::underwater::LiveActivityKind::UsingTool => Self::Tools,
            crate::tui::underwater::LiveActivityKind::UsingSubagents => Self::Subagents,
            crate::tui::underwater::LiveActivityKind::Verifying => Self::Verifying,
            _ => Self::Baseline,
        }
    }

    /// Ocean-clock speed factor: thinking reads as the slow deep, tool work
    /// as the fast current.
    fn speed(self) -> f32 {
        match self {
            Self::Reasoning => 0.6,
            Self::Reading => 0.8,
            Self::Tools | Self::Subagents => 1.25,
            Self::Verifying => 1.0,
            Self::Baseline => 1.0,
        }
    }

    fn scaled_time_ms(self, elapsed_ms: u128) -> u128 {
        let speed = self.speed();
        if (speed - 1.0).abs() < f32::EPSILON {
            elapsed_ms
        } else {
            ((elapsed_ms as f64) * f64::from(speed)) as u128
        }
    }

    /// A sub-agent run surfaces as a pod: the completion cameo becomes three
    /// whales at staggered offsets instead of one.
    fn pod_cameo(self) -> bool {
        self == Self::Subagents
    }

    fn pod_offsets(self) -> &'static [i16] {
        if self.pod_cameo() { &[-5, 0, 5] } else { &[0] }
    }
}

const WHALE_CAMEO_MS: u128 = 2_400;

/// Render ambient life into empty water cells of `area`.
///
/// Returns per-frame budget counters for tests and debug tooling; the
/// counting itself is a few `u32` increments, never an allocation.
#[allow(clippy::too_many_arguments)]
pub fn render_ambient_life(
    area: Rect,
    buf: &mut Buffer,
    inks: (Color, Color),
    lines: &[Line<'static>],
    elapsed_ms: u128,
    presence: f32,
    cursor: AmbientCursor,
    whale: WhaleCameo,
    activity: AmbientActivity,
) -> AmbientFrameStats {
    if area.width < AMBIENT_MIN_WIDTH || area.height < AMBIENT_MIN_HEIGHT {
        return AmbientFrameStats::default();
    }

    // Activity shifts the ocean clock: reading feels like the deep (slow
    // drift), tool work like a brighter current (faster), verification stays
    // on the metered pulse. Density stays bounded by MAX_FRAME_MARKS.
    let elapsed_ms = activity.scaled_time_ms(elapsed_ms);

    let density = LifeDensity::from_area(area);
    let mut stats = AmbientFrameStats::default();
    // Positions always ride the live monotonic clock; `presence` fades the
    // marks in and out, so the animated/static boundary eases instead of
    // snapping fish between t=0 and their mid-path positions.
    let frame = build_frame_marks(
        area, elapsed_ms, density, lines, cursor, whale, activity, &mut stats,
    );
    paint_marks(area, buf, inks, lines, &frame, presence, &mut stats);
    stats
}

#[allow(clippy::too_many_arguments)]
fn build_frame_marks(
    area: Rect,
    elapsed_ms: u128,
    density: LifeDensity,
    lines: &[Line<'static>],
    cursor: AmbientCursor,
    whale: WhaleCameo,
    activity: AmbientActivity,
    stats: &mut AmbientFrameStats,
) -> FrameMarks {
    let mut marks = Vec::with_capacity(48);
    let t = elapsed_ms;

    // Where the water is. The old rule was a guess at where the composition
    // sat — fifths of the field — and it was wrong on every real screen: at
    // 80×24 it reserved two rows in the middle of the field while the
    // wordmark, caption, and invitation lived three rows lower, so a fish
    // surfaced in the one-row gap between the caption and the invitation.
    // Now the field is measured, not guessed: [`is_open_water`] asks the
    // rendered lines directly.
    let water = |x: u16, y: u16, width: u16| is_open_water(lines, x, y, width);

    // --- One loose fish school along the floor ---
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
    let (cycle_index, cycle_step) = (
        school_clock / cycle_ms,
        ((school_clock % cycle_ms) / SCHOOL_CELL_MS) as i32,
    );
    let swims_right = school_swims_right(cycle_index);
    // The school has one home: the deep water just off the floor. It used to
    // alternate between an upper and a lower band, which is most of why the
    // aquarium read as decoration sprinkled over the whole field instead of
    // as depth beneath it. Direction still alternates — that is the part a
    // viewer reads as "the fish came back" — but the band does not.
    let anchor_y = school_band_row(area);
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
        let bob = sine_bob(t, 3_400 + (m as u128) * 640, 1);
        let y_i32 = i32::from(anchor_y) + i32::from(*dy) + i32::from(bob);
        // Fish dart sideways away from the scatter anchor (nearby only).
        if let Some(flee_ms) = cursor.flee_elapsed_ms {
            let flee = i32::from(fish_flee_offset(flee_ms));
            if x_i32.abs_diff(i32::from(ptr)) < 16 && y_i32.abs_diff(i32::from(ptr_y)) < 6 {
                // Horizontal only. The old ±1 row kick pushed the outer
                // fish off the school's band and straight into the row the
                // composition had already claimed, so a scatter punched a
                // hole in the wedge exactly when the eye was on it.
                if x_i32 >= i32::from(ptr) {
                    x_i32 += flee;
                } else {
                    x_i32 -= flee;
                }
            }
        }
        let max_x = i32::from(area.width.saturating_sub(body_w));
        let max_y = i32::from(area.height.saturating_sub(1));
        if x_i32 < 0 || x_i32 > max_x || y_i32 < 0 || y_i32 > max_y {
            continue; // off-screen while wrapping
        }
        let y = y_i32 as u16;
        // Never swim through the composition or the row of air around it.
        if !water(x_i32 as u16, y, body_w) {
            continue;
        }
        let brightness = FISH_BRIGHTNESS_FLOOR
            + (1.0 - FISH_BRIGHTNESS_FLOOR)
                * wave01(t, FISH_WAVE_MS, (m as u128).saturating_mul(320));
        marks.push(AmbientMark {
            x: x_i32 as u16,
            y,
            glyph: body,
            jellyfish: None,
            depth: if m == 0 {
                Depth::Foreground
            } else {
                Depth::Midground
            },
            style_mod: None,
            brightness: Some(brightness),
        });
    }

    // --- Jellyfish: a pulsing dome with lagging tentacles ---
    // Two dome rows (arc + bell rim) over a row of swaying
    // tentacles. The dome opens and closes on a slow floor-bounded sin²;
    // the tentacles repeat the pulse ~350 ms later and sway out of phase
    // with each other — the lag is what sells "jellyfish". Rich/Normal get
    // the full 5-cell dome; Sparse (narrow) swaps in a compact 3-cell one — a
    // real fallback silhouette, not just fewer jellies. Both hang two
    // tentacles. They drift slowly upward through a side lane of the deep
    // water and vanish before the rise would reach the composition.
    //
    // It only visits water deep enough to hold it: three rows of silhouette,
    // a row of clear water, and the school's own band, measured up from the
    // floor. At 80×24 the composition leaves four rows of water and the
    // jellyfish used to land inside the school — a five-cell pulsing
    // silhouette and a wedge of fish sharing four rows of a 24-row terminal
    // is the definition of not earning the space. Below the budget it simply
    // does not come up.
    let jellyfish_count = density.jellyfish_count();
    for j in 0..jellyfish_count {
        let phase = 3_100u128.saturating_add((j as u128) * 4_700);
        let lane_x = if j % 2 == 0 {
            area.width.saturating_mul(5) / 6
        } else {
            area.width / 6
        };
        let wobble = sine_bob(t, 5_200 + phase, 1);
        let compact = density == LifeDensity::Sparse;
        let (dome_top, dome_skirt, tentacle_cols): (&[&str], &[&str], &[u16]) = if compact {
            (JELLY_DOME_TOP_COMPACT, JELLY_DOME_SKIRT_COMPACT, &[0, 2])
        } else {
            // Two tentacles hanging from the rim, not three abreast. Three
            // adjacent one-cell strokes spend most of their sway table
            // rendering as `||\` or `|||` — a solid bar of punctuation under
            // the bell, which is what the dogfood frame actually showed.
            (JELLY_DOME_TOP_FRAMES, JELLY_DOME_SKIRT_FRAMES, &[1, 3])
        };
        let dome_w = dome_top[0].len() as u16; // ASCII frames: len == width
        let x = lane_x
            .saturating_add(wobble)
            .min(area.width.saturating_sub(dome_w + 1));
        if deep_water_rows(area, lines, x, dome_w) < JELLY_MIN_DEEP_ROWS {
            continue;
        }
        // A visit is a short, slow rise near the floor followed by a long
        // absence: the jelly climbs [`JELLY_VISIT_ROWS`] rows and then spends
        // the rest of the cycle out of sight. Rows are discrete cells, so the
        // per-row dwell stays long — a jellyfish should read as drifting, not
        // as stepping.
        let rise_period = JELLY_RISE_ROW_MS.saturating_add((j as u128) * JELLY_RISE_ROW_STAGGER_MS);
        let slot = (t.saturating_add(phase) / rise_period) % JELLY_VISIT_CYCLE_SLOTS;
        if slot >= u128::from(JELLY_VISIT_ROWS) {
            continue; // still down in the dark between visits
        }
        let risen = slot as u16;
        let y = area
            .height
            .saturating_sub(JELLY_FLOOR_GAP)
            .saturating_sub(risen);
        if y == 0 || !water(x, y, dome_w) {
            continue;
        }
        let dome_brightness = jelly_glow(wave01(t, JELLY_PULSE_MS, phase));
        let tentacle_brightness = jelly_glow(wave01(
            t.saturating_sub(JELLY_TENTACLE_LAG_MS),
            JELLY_PULSE_MS,
            phase,
        ));
        // The dome opens/closes on the same clock as its glow; the parked
        // pose holds the half-pulsed (contracted) frame.
        let pulse_frame = usize::from(wave01(t, JELLY_PULSE_MS, phase) > 0.5);
        let skirt_row = y.saturating_add(1);
        let tentacle_row = y.saturating_add(2);
        // Treat the silhouette as one visual unit. The former per-row quiet
        // band checks deliberately allowed the dome, skirt, or tentacles to
        // disappear independently, which is exactly the broken punctuation
        // visible in the v0.9.2 dogfood screenshot.
        if tentacle_row >= area.height
            || ![y, skirt_row, tentacle_row]
                .into_iter()
                .all(|row| water(x, row, dome_w))
        {
            continue;
        }
        for (row, glyph) in [
            (y, dome_top[pulse_frame]),
            (skirt_row, dome_skirt[pulse_frame]),
        ] {
            marks.push(AmbientMark {
                x,
                y: row,
                glyph,
                jellyfish: Some(j),
                // Background ink, same as the tentacles: the dome used to sit
                // a layer nearer than everything else in the side lanes,
                // which is most of why it drew the eye.
                depth: Depth::Background,
                style_mod: None,
                brightness: Some(dome_brightness),
            });
        }
        for (col, &dx) in tentacle_cols.iter().enumerate() {
            // Each column runs the sway table with its own phase offset
            // so the trio lags left-to-right; the parked pose holds a
            // mid-sway frame.
            let frame = t
                .saturating_add(phase)
                .saturating_add((col as u128) * JELLY_TENTACLE_PHASE_STEP_MS)
                / JELLY_TENTACLE_SWAY_MS;
            let sway = JELLY_TENTACLE_FRAMES[(frame as usize) % JELLY_TENTACLE_FRAMES.len()];
            marks.push(AmbientMark {
                x: x.saturating_add(dx),
                y: tentacle_row,
                glyph: sway,
                jellyfish: Some(j),
                depth: Depth::Background,
                style_mod: None,
                brightness: Some(tentacle_brightness),
            });
        }
    }

    // --- Rising bubble streams: a short run off the floor, then dissolve ---
    // A bubble used to travel the whole column and then clamp at the top of
    // the field, where it parked as a single unattached speck for the rest of
    // its cycle — the `·` sitting at column 11 doing nothing in the 80×24
    // frame. It now rises [`BUBBLE_MAX_RISE_ROWS`] rows from the floor,
    // grows, and fades out, which is both what a bubble does and a reason for
    // it to be exactly where it is.
    for b in 0..density.bubble_streams() {
        let phase = (b as u128).saturating_mul(1_900);
        // Edge columns — avoid center brand.
        let column = if b % 2 == 0 {
            area.width / 8
        } else {
            area.width.saturating_mul(7) / 8
        };
        let rise_period = BUBBLE_RISE_MS.saturating_add(phase % 900);
        let cycle = (t.saturating_add(phase) % rise_period) as f64 / rise_period as f64;
        let boost = if cursor.flee_elapsed_ms.is_some() && column.abs_diff(ptr) < 10 {
            2
        } else {
            0
        };
        let rise = ((cycle * f64::from(BUBBLE_MAX_RISE_ROWS)) as u16)
            .saturating_add(boost)
            .min(BUBBLE_MAX_RISE_ROWS);
        let y = area.height.saturating_sub(2).saturating_sub(rise);
        if !water(column, y, 1) {
            continue;
        }
        // Size is a function of height risen, not of the clock: the old
        // `["·", "˚", "·", "°"]` table swapped glyph every 320 ms in place,
        // which is a flicker in peripheral vision rather than a rise.
        let glyph = bubble_glyph(rise);
        let brightness = glint01(
            t,
            BUBBLE_GLINT_MS.saturating_add(phase % 700),
            600,
            BUBBLE_BRIGHTNESS_FLOOR,
            phase,
        ) * bubble_dissolve(rise);
        marks.push(AmbientMark {
            x: column.min(area.width.saturating_sub(1)),
            y,
            glyph,
            jellyfish: None,
            depth: Depth::Foreground,
            style_mod: None,
            brightness: Some(brightness),
        });
    }

    // --- Rare whale cameo (completion only) ---
    if let Some(cameo_ms) = whale.elapsed_ms.filter(|ms| *ms < WHALE_CAMEO_MS) {
        for (pod_index, offset) in activity.pod_offsets().iter().enumerate() {
            // Pod members ride the same cameo breath, staggered so a sub-agent
            // completion reads as a group surfacing rather than one whale.
            let pod_cameo_ms = cameo_ms.saturating_add((pod_index as u128) * 240);
            if pod_cameo_ms >= WHALE_CAMEO_MS {
                continue;
            }
            let phase = whale_cameo_phase(pod_cameo_ms);
            if phase == WhaleCameoPhase::Hidden {
                continue;
            }
            let ax = whale
                .anchor_x
                .saturating_add_signed(*offset)
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
                    jellyfish: None,
                    depth: Depth::Foreground,
                    style_mod: None,
                    brightness: None,
                });
                if phase == WhaleCameoPhase::Spout && ay > 0 {
                    marks.push(AmbientMark {
                        x: ax.saturating_add(1).min(area.width.saturating_sub(1)),
                        y: ay.saturating_sub(1),
                        glyph: "˚",
                        jellyfish: None,
                        depth: Depth::Foreground,
                        style_mod: Some(Modifier::DIM),
                        brightness: None,
                    });
                }
            }
        }
    }

    stats.marks_built = marks.len() as u32;
    FrameMarks { marks }
}

/// Loose diagonal wedge for the school: `(row_offset, columns_behind_lead)`.
/// The slight row spread is what makes it read as a school, not a text row.
///
/// Three rows, not five. The ±2 rows put the wedge across a fifth of a 24-row
/// terminal, which reads as fish scattered over the screen rather than as one
/// shoal; at ±1 (plus each fish's own bob) the school still has depth but
/// stays a single object the eye can take in at once.
const SCHOOL_WEDGE: &[(i16, u16)] = &[(0, 0), (-1, 4), (1, 6), (-1, 9), (1, 11), (0, 14), (-1, 17)];

/// Rows between the school's centre line and the bottom of the field. With the
/// ±1 wedge and a one-row bob the shoal occupies `height-4 ..= height-1`: the
/// deep water, clear of anything the composition is using.
const SCHOOL_FLOOR_GAP: u16 = 3;

/// The row the school centres on, in field-local coordinates. Public so the
/// compositor can aim a scatter at the shoal instead of guessing where it is.
#[must_use]
pub fn school_band_row(area: Rect) -> u16 {
    area.height.saturating_sub(SCHOOL_FLOOR_GAP)
}

/// Wall-clock milliseconds per column of school travel (~2.6 cells/s).
const SCHOOL_CELL_MS: u128 = 380;
/// Travelling brightness-wave period through the wedge.
const FISH_WAVE_MS: u128 = 2_200;
/// Fish are small: never let one sink into the gradient.
const FISH_BRIGHTNESS_FLOOR: f32 = 0.45;

/// Lead fish silhouettes (ASCII only — width == len). Members drop the eye.
const LEAD_FISH_RIGHT: &str = "><o>";
const LEAD_FISH_LEFT: &str = "<o><";

/// Jellyfish silhouette frames — pure ASCII by construction so the
/// ascii_safe tier needs no fallback mapping for them (len == width).
///
/// Full dome (Rich/Normal), two rows with an open/closed pulse pair: a
/// rounded arc over the bell's rim.
///
/// The skirt is the bell's lower rim and nothing else: it carries the pulse by
/// flaring (`\` `/`) and contracting (`(` `)`), the way a real bell swims. It
/// holds no interior glyphs on purpose — an earlier pair put marks inside the
/// rim (`(v_v)` / `(v.v)`), which read as two eyes and a mouth. The motion the
/// silhouette is meant to sell lives in the tentacle row below, not in the
/// skirt.
///
/// Both contracted frames are left-right symmetric on purpose. The former
/// `.'-.'` and `'.'` were not — a dot on one side and an apostrophe on the
/// other — and an asymmetric five-cell arc does not read as a bell at all; in
/// the 80×24 dogfood frame it read as three unrelated rows of punctuation.
const JELLY_DOME_TOP_FRAMES: &[&str] = &[".-~-.", ".'-'."];
const JELLY_DOME_SKIRT_FRAMES: &[&str] = &["\\___/", "(___)"];
/// Compact dome for the Sparse (narrow) tier: same two-row read at 3 cells.
const JELLY_DOME_TOP_COMPACT: &[&str] = &[".-.", "'-'"];
const JELLY_DOME_SKIRT_COMPACT: &[&str] = &["\\_/", "(_)"];
/// Tentacle sway frames (all width-1). Each column runs the same table with
/// a phase offset so the pair lags instead of strobing in sync.
const JELLY_TENTACLE_FRAMES: &[&str] = &["|", "/", "|", "\\"];

/// How far sideways a jellyfish may dodge to clear transcript text before it
/// is withheld for the frame instead.
///
/// Placement is a pure function of the text under the silhouette, so during a
/// fast stream it is effectively a function of token throughput: a growing
/// line pushes the anchor one column per character, and a wrap or a scroll
/// collapses that row's occupied bounds and snaps the anchor back tens of
/// columns in a single frame. On screen that reads as teleporting, and it only
/// shows up on models fast enough to change those bounds every frame — which
/// is why slow providers never surfaced it.
///
/// Bounding the dodge keeps the behavior the silhouette was actually given
/// (ease around a word that happens to brush its lane) and turns everything
/// larger into the same quiet withhold the fish already use. Worst-case
/// frame-to-frame movement is therefore `2 * JELLY_MAX_TEXT_DODGE_COLS`, at
/// the single moment a left-hand candidate overtakes a right-hand one.
const JELLY_MAX_TEXT_DODGE_COLS: u16 = 3;

// --- Jellyfish rarity ------------------------------------------------------
// The jellyfish is the loudest thing in the water: a five-cell silhouette that
// changes glyph as it pulses, parked in a side lane. Before v0.9.4 it was also
// permanently resident, which is the combination that made it obnoxious rather
// than incidental. Everything below is one knob with one stated intent, so the
// balance can be retuned without re-deriving it from the motion code.

/// Wall-clock milliseconds a jellyfish spends on each row of its rise
/// (~9.4 s). A row step is a discrete one-cell jump, so the dwell has to stay
/// long or the rise reads as stepping rather than drifting.
const JELLY_RISE_ROW_MS: u128 = 9_400;
/// Per-jelly rise-rate stagger, so two jellyfish (should a tier ever want
/// them again) can never step in lockstep.
const JELLY_RISE_ROW_STAGGER_MS: u128 = 1_400;
/// Rows climbed in a single visit — about 56 s of presence.
const JELLY_VISIT_ROWS: u16 = 6;
/// Rows between the jellyfish's dome and the bottom of the field. The
/// silhouette is three rows tall, so this leaves exactly one row of clear
/// water between its tentacles and the top of the school's band — the
/// jellyfish is a visitor in the same water, not a passenger on the shoal.
const JELLY_FLOOR_GAP: u16 = 8;
/// Unbroken water rows (measured up from the floor) a jellyfish needs before
/// it will surface at all: its own three rows, the gap, and the school's band.
/// Same number as [`JELLY_FLOOR_GAP`] by construction — the dome's row is the
/// deepest row it touches.
const JELLY_MIN_DEEP_ROWS: u16 = JELLY_FLOOR_GAP;
/// Row-slots in one full visit cycle. Slots at or past [`JELLY_VISIT_ROWS`]
/// are spent out of sight, and that gap is *the* rarity knob: at 32 slots the
/// cycle is ~5 min and a jellyfish is present under a fifth of the time —
/// occasionally noticed, never resident. Raise it to make them rarer; lower
/// it to bring them back. It must stay `> JELLY_VISIT_ROWS` or the jelly
/// becomes permanent again.
const JELLY_VISIT_CYCLE_SLOTS: u128 = 32;

// --- Jellyfish motion and glow ---------------------------------------------

/// Dome pulse period. Slow on purpose: a pulse fast enough to notice in
/// peripheral vision is a pulse that interrupts reading.
const JELLY_PULSE_MS: u128 = 5_200;
/// The tentacles repeat the dome pulse this much later. Held at ~12% of
/// [`JELLY_PULSE_MS`] — the lag is what sells "jellyfish", so it scales with
/// the pulse rather than staying an absolute number.
const JELLY_TENTACLE_LAG_MS: u128 = 620;
/// Wall-clock milliseconds per tentacle sway frame.
const JELLY_TENTACLE_SWAY_MS: u128 = 2_600;
/// Per-column sway phase offset, so the two tentacles never move in sync.
/// Keep this a non-divisor of [`JELLY_TENTACLE_SWAY_MS`] or the pair strobes.
const JELLY_TENTACLE_PHASE_STEP_MS: u128 = 700;
/// Dimmest point of the pulse: still legible against the water, no lower.
const JELLY_BRIGHTNESS_FLOOR: f32 = 0.28;
/// Brightest point of the pulse. Deliberately well short of full ink — the
/// jellyfish used to swing floor-to-1.0, and that swing (not its presence)
/// is what pulled the eye off the transcript.
const JELLY_BRIGHTNESS_CEIL: f32 = 0.62;

/// Map a `[0, 1]` pulse onto the jellyfish's shallow glow band.
#[must_use]
fn jelly_glow(pulse: f32) -> f32 {
    JELLY_BRIGHTNESS_FLOOR + (JELLY_BRIGHTNESS_CEIL - JELLY_BRIGHTNESS_FLOOR) * pulse
}

/// Bubbles stay mostly steady with occasional glints, not a constant wave.
const BUBBLE_BRIGHTNESS_FLOOR: f32 = 0.55;
/// Rows a bubble climbs before it dissolves. Short on purpose: a bubble that
/// crosses the whole field is a moving speck with no source and no end.
const BUBBLE_MAX_RISE_ROWS: u16 = 5;
/// Wall-clock milliseconds for one bubble to make that climb.
const BUBBLE_RISE_MS: u128 = 3_200;
/// Base period of the raised-cosine glint.
const BUBBLE_GLINT_MS: u128 = 2_600;
/// How much of its brightness a bubble keeps at the top of its rise.
const BUBBLE_DISSOLVE_CEIL: f32 = 0.25;

/// Bubbles grow as they rise. Keyed to height, never to the clock.
#[must_use]
fn bubble_glyph(rise: u16) -> &'static str {
    match rise {
        0..=1 => "·",
        2..=3 => "˚",
        _ => "°",
    }
}

/// Linear fade across the rise: full at the floor, nearly gone at the top.
#[must_use]
fn bubble_dissolve(rise: u16) -> f32 {
    let span = f32::from(BUBBLE_MAX_RISE_ROWS.max(1));
    let remaining = f32::from(BUBBLE_MAX_RISE_ROWS.saturating_sub(rise)) / span;
    BUBBLE_DISSOLVE_CEIL + (1.0 - BUBBLE_DISSOLVE_CEIL) * remaining
}

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

/// Rows of clear air the composition keeps on each side of every line it
/// writes. One row is enough: it is the difference between a fish swimming
/// *behind* a block of text and a fish surfacing in the gap between two of its
/// lines, which is what the 80×24 frame showed between the caption and the
/// invitation.
const TEXT_CLEARANCE_ROWS: u16 = 1;

/// True when the horizontal span at `(x, y)` — and the same span on every row
/// within [`TEXT_CLEARANCE_ROWS`] — carries no rendered text. One column of
/// horizontal air is reserved on both sides so a fish never touches the prose,
/// while short left-aligned transcript lines still leave real water to their
/// right.
#[must_use]
fn is_open_water(lines: &[Line<'_>], x: u16, y: u16, width: u16) -> bool {
    let first = usize::from(y.saturating_sub(TEXT_CLEARANCE_ROWS));
    let last = usize::from(y.saturating_add(TEXT_CLEARANCE_ROWS));
    !(first..=last).any(|row| {
        lines
            .get(row)
            .and_then(occupied_text_bounds)
            .is_some_and(|(start, end)| span_touches_text(x, width, start, end))
    })
}

#[must_use]
fn span_touches_text(x: u16, width: u16, start: usize, end: usize) -> bool {
    usize::from(x) < end.saturating_add(1)
        && usize::from(x).saturating_add(usize::from(width)) > start.saturating_sub(1)
}

/// Unbroken open-water rows measured up from the bottom of the field: how much
/// deep water the composition has left for the aquarium to live in.
#[must_use]
fn deep_water_rows(area: Rect, lines: &[Line<'_>], x: u16, width: u16) -> u16 {
    let mut rows = 0u16;
    let mut y = area.height;
    while y > 0 {
        y -= 1;
        if !is_open_water(lines, x, y, width) {
            break;
        }
        rows = rows.saturating_add(1);
    }
    rows
}

fn paint_marks(
    area: Rect,
    buf: &mut Buffer,
    inks: (Color, Color),
    lines: &[Line<'static>],
    frame: &FrameMarks,
    presence: f32,
    stats: &mut AmbientFrameStats,
) {
    if presence <= 0.0 {
        // Fully static water: nothing to paint (all marks invisible).
        return;
    }
    let presence = presence.clamp(0.0, 1.0);
    #[derive(Clone, Copy)]
    enum SkipReason {
        Text,
        Clipped,
    }

    #[derive(Clone, Copy)]
    enum Placement {
        Anchor { original: u16, placed: u16 },
        Skip(SkipReason),
    }
    let mut placements: [Option<Placement>; 2] = [None, None];
    let population_overflow = frame
        .marks
        .iter()
        .filter_map(|mark| mark.jellyfish)
        .any(|jellyfish| jellyfish >= placements.len());
    debug_assert!(
        !population_overflow,
        "jellyfish population exceeded its bound"
    );
    for (jellyfish, placement) in placements.iter_mut().enumerate() {
        let marks = || {
            frame
                .marks
                .iter()
                .filter(move |mark| mark.jellyfish == Some(jellyfish))
        };
        let Some(original) = marks().map(|mark| mark.x).min() else {
            continue;
        };
        let mut group_end = 0u16;
        for mark in marks() {
            let offset = mark.x.saturating_sub(original);
            let width = u16::try_from(UnicodeWidthStr::width(mark.glyph)).unwrap_or(u16::MAX);
            group_end = group_end.max(offset.saturating_add(width));
        }
        let Some(right_edge) = area.width.checked_sub(group_end) else {
            *placement = Some(Placement::Skip(SkipReason::Clipped));
            continue;
        };

        let mut best: Option<(u16, u16)> = None;
        let mut consider = |candidate: i64| {
            let Ok(candidate) = u16::try_from(candidate) else {
                return;
            };
            // Bounded dodge. Anything further than the cap is a relocation
            // rather than a drift, so it is refused here and the silhouette
            // is withheld instead — see [`JELLY_MAX_TEXT_DODGE_COLS`].
            let dodge = candidate.abs_diff(original);
            if dodge > JELLY_MAX_TEXT_DODGE_COLS {
                return;
            }
            let fits = candidate <= right_edge
                && marks().all(|mark| {
                    let x = candidate.saturating_add(mark.x.saturating_sub(original));
                    let width =
                        u16::try_from(UnicodeWidthStr::width(mark.glyph)).unwrap_or(u16::MAX);
                    is_open_water(lines, x, mark.y, width)
                });
            if fits {
                let ranked = (dodge, candidate);
                if best.is_none_or(|current| ranked < current) {
                    best = Some(ranked);
                }
            }
        };
        consider(i64::from(original));
        consider(0);
        consider(i64::from(right_edge));
        for mark in marks() {
            let offset = mark.x.saturating_sub(original);
            let mark_end = offset.saturating_add(
                u16::try_from(UnicodeWidthStr::width(mark.glyph)).unwrap_or(u16::MAX),
            );
            let first = usize::from(mark.y.saturating_sub(TEXT_CLEARANCE_ROWS));
            let last = usize::from(mark.y.saturating_add(TEXT_CLEARANCE_ROWS));
            for (start, end) in
                (first..=last).filter_map(|row| lines.get(row).and_then(occupied_text_bounds))
            {
                if let Ok(start) = i64::try_from(start) {
                    consider(start - 1 - i64::from(mark_end));
                }
                if let Ok(end) = i64::try_from(end) {
                    consider(end + 1 - i64::from(offset));
                }
            }
        }
        *placement = Some(match best {
            Some((_, placed)) => Placement::Anchor { original, placed },
            None => Placement::Skip(SkipReason::Text),
        });
    }

    for mark in &frame.marks {
        let mark_placement = mark
            .jellyfish
            .map(|index| placements.get(index).copied().flatten());
        let (mark_x, preflighted) = match mark_placement {
            Some(None) => {
                stats.marks_clipped += 1;
                continue;
            }
            Some(Some(Placement::Anchor { original, placed })) => (
                placed
                    .checked_add(mark.x.saturating_sub(original))
                    .expect("preflight accepted a clipped jellyfish"),
                true,
            ),
            Some(Some(Placement::Skip(SkipReason::Text))) => {
                stats.marks_skipped_text += 1;
                continue;
            }
            Some(Some(Placement::Skip(SkipReason::Clipped))) => {
                stats.marks_clipped += 1;
                continue;
            }
            None => (mark.x, false),
        };
        if !preflighted {
            let mark_width = UnicodeWidthStr::width(mark.glyph);
            // Clipped is checked before text collision so a mark that fails
            // both is charged to the bound it could never satisfy.
            if mark_x.saturating_add(mark_width as u16) > area.width {
                stats.marks_clipped += 1;
                continue;
            }
            if !is_open_water(
                lines,
                mark_x,
                mark.y,
                u16::try_from(mark_width).unwrap_or(u16::MAX),
            ) {
                stats.marks_skipped_text += 1;
                continue;
            }
        }
        stats.marks_painted += 1;
        let ink = if mark.depth.ink_index() == 1 {
            inks.1
        } else {
            inks.0
        };
        for (offset, ch) in mark.glyph.chars().enumerate() {
            let cell = &mut buf[(area.x + mark_x + offset as u16, area.y + mark.y)];
            // Glow language: lerp the mark's ink up from the water the cell
            // already sits in, at the entity's time-varying brightness. The
            // overall lerp is additionally scaled by life presence so marks
            // fade in/out with the animated/static boundary.
            let fg = match (mark.brightness, cell.style().bg) {
                (Some(amount), Some(water)) => {
                    ocean::mix_colors(water, ink, (amount * presence).clamp(0.0, 1.0))
                }
                (Some(amount), None) => ocean::scale_color(ink, amount.clamp(0.0, 1.0).max(0.4)),
                (None, Some(water)) => ocean::mix_colors(water, ink, presence),
                (None, None) => ocean::scale_color(ink, presence),
            };
            let mut style = Style::default().fg(fg);
            if let Some(m) = mark.style_mod {
                style = style.add_modifier(m);
            }
            cell.set_symbol(&ch.to_string());
            cell.set_style(style);
            stats.cells_written += 1;
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
    //
    // The light stops where the composition starts. Sunlight raking across
    // the rows a wordmark is sitting in is the same failure as a fish
    // swimming through them, just quieter, and it costs nothing to measure:
    // the surface band is clipped to the first row that carries text.
    let ceiling = (0..area.height)
        .find(|row| {
            lines
                .get(usize::from(*row))
                .and_then(occupied_text_bounds)
                .is_some()
        })
        .unwrap_or(area.height);
    let band = (area.height / 3).max(2).min(ceiling);
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
            let cell = &mut buf[(area.x + local_x, area.y + local_y)];
            // Soften toward ambient ink without replacing semantic glyphs.
            if cell.symbol() == " " || cell.symbol().is_empty() {
                // Sunlight dissolves with depth instead of stopping: full
                // amplitude at the surface easing to zero at the band's
                // floor. The former hard cutoff at `band` drew a visible
                // horizontal line across tall windows.
                let depth_fade = 1.0 - f32::from(local_y) / f32::from(band.max(1));
                let shimmer = ocean::scale_color(
                    row_bg,
                    caustic_brightness(elapsed_ms, local_x, local_y, depth_fade * depth_fade),
                );
                cell.set_bg(shimmer);
            }
        }
    }
}

/// Continuous travelling caustic. The former `(elapsed / 80) % 12` mask
/// toggled cells fully on/off at 12.5 Hz; truecolor made that quantization look
/// like dropped frames. A narrow cosine crest preserves the same sparse light
/// band while cross-fading every sampled cell between frames.
fn caustic_brightness(elapsed_ms: u128, local_x: u16, local_y: u16, depth_fade: f32) -> f32 {
    const CYCLE_MS: f64 = 960.0;
    const SPATIAL_SLOTS: f64 = 4.0;
    let time = (elapsed_ms % CYCLE_MS as u128) as f64 / CYCLE_MS;
    // The sampled grid advances by three terminal columns. Four grid phases
    // therefore preserve the old 12-column repeat instead of stretching the
    // caustic topology while changing only its temporal interpolation.
    let slot = (u32::from(local_x / 3) + u32::from(local_y)) % 4;
    let phase = (time + f64::from(slot) / SPATIAL_SLOTS) * std::f64::consts::TAU;
    let crest = ((phase.cos() + 1.0) * 0.5).powi(8);
    1.0 + 0.08 * (crest as f32) * depth_fade.clamp(0.0, 1.0)
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
        // The breath and completion fade are continuous. Bucket at a 60 FPS
        // floor so Ghostty's smooth-motion lane is not quantized back to the
        // old 80 ms atmosphere cadence; slower terminals still call this only
        // when they actually draw.
        let bucket = elapsed_ms / 16;
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
#[path = "ambient_life/tests.rs"]
mod tests;
