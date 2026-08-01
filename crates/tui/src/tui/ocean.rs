//! Terminal-native underwater field for the Codewhale transcript.
//!
//! The field is atmosphere, never content: ordinary shell cells share its
//! water column while semantic surfaces such as selections, errors, and code
//! keep their own backgrounds. Reduced motion freezes the field but does not
//! remove it, so choosing an underwater treatment always has a visible result.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

use crate::palette::{PaletteMode, UiTheme};
use crate::tui::underwater::ShellPhase;

/// Appearance treatment for the underwater shell.
///
/// Parsed once from persisted settings so rendering and scheduling code can
/// branch on typed state instead of scattered string comparisons. Treatment
/// is appearance only: ambient life belongs to every underwater treatment,
/// while motion is governed separately by `low_motion`/`fancy_animations`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OceanTreatment {
    /// State-reactive water column painted from the theme's [`OceanRamp`].
    #[default]
    Ombre,
    /// Plain theme surface with the same state grammar and ambient life.
    Flat,
    /// Legacy full-chrome compatibility shell. Persisted settings normalize
    /// unknown values to ombre, so this is reachable only through explicit
    /// internal selection (tests and future compatibility wiring).
    Classic,
}

impl OceanTreatment {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let value = value.trim();
        if value.eq_ignore_ascii_case("flat") {
            Self::Flat
        } else if value.eq_ignore_ascii_case("classic") {
            Self::Classic
        } else {
            Self::Ombre
        }
    }

    #[must_use]
    pub fn is_ombre(self) -> bool {
        self == Self::Ombre
    }

    #[must_use]
    pub fn is_flat(self) -> bool {
        self == Self::Flat
    }

    #[must_use]
    pub fn is_classic(self) -> bool {
        self == Self::Classic
    }

    /// Every underwater treatment keeps idle ambient life; only the legacy
    /// classic shell stays still. Flat means a plain surface, not a lifeless
    /// ocean, and Terminal-owned backgrounds still carry foreground life.
    #[must_use]
    pub fn supports_ambient_life(self) -> bool {
        !self.is_classic()
    }
}

/// Minimum empty-water size that earns decorative ambient life. Below this,
/// content and controls own every cell. Shared by the renderer and the idle
/// animation scheduler so redraws are never scheduled for invisible life.
/// Lowered in v0.9.1 so smaller windows still retain some life.
pub const AMBIENT_MIN_WIDTH: u16 = 40;
pub const AMBIENT_MIN_HEIGHT: u16 = 10;

/// Ambient-life inks for a theme, independent of the ombre ramp. Fish use two
/// sunk sky-blue shades so seafoam remains reserved for live work.
#[must_use]
pub fn ambient_inks(theme: &UiTheme) -> (Color, Color) {
    let sky = rgb(theme.info).unwrap_or((106, 174, 242));
    match rgb(theme.surface_bg) {
        Some(base) => (color(mix(sky, base, 0.42)), color(mix(sky, base, 0.28))),
        None => (theme.info, theme.info),
    }
}

/// Length of the completion breath (the column's settle flourish), ms.
pub const COMPLETION_BREATH_MS: u128 = 800;

/// Extra ms after the breath during which ambient life eases out of view.
pub const SETTLE_MS: u128 = 600;
pub(crate) const COMPLETION_SETTLE_MS: u128 = COMPLETION_BREATH_MS + SETTLE_MS;

/// Ms over which animated life ramps in when a working phase begins.
pub const RAMP_MS: u128 = 450;

/// Smoothstep easing: 0 at t=0, 1 at t=1, zero velocity at both ends.
#[must_use]
pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Life presence (0..=1) as a pure function of the monotonic clocks. There is
/// deliberately NO per-frame mutable state here: the same inputs always yield
/// the same output, which keeps ambient-life renders deterministic.
///
/// Rules:
/// - A turn just ended (`completion_elapsed_ms` within the breath) holds full
///   presence so ambient life keeps swimming through the settle flourish.
/// - After the breath, presence eases out over [`SETTLE_MS`] so the water
///   settles instead of snapping from animated to frozen.
/// - Browsing history or the pristine empty state is user-driven: full
///   presence immediately.
/// - A Working/Verifying phase ramps in from `turn_elapsed_ms` over
///   [`RAMP_MS`], giving bursty fast streams a calm, bounded onset.
/// - Everything else is fully static.
#[must_use]
pub fn life_presence(
    completion_elapsed_ms: Option<u128>,
    turn_elapsed_ms: Option<u128>,
    animated: bool,
    browsing_history: bool,
    empty_state: bool,
) -> f32 {
    if let Some(elapsed) = completion_elapsed_ms {
        if elapsed < COMPLETION_BREATH_MS {
            return 1.0;
        }
        let t = (elapsed - COMPLETION_BREATH_MS) as f32 / SETTLE_MS as f32;
        return 1.0 - smoothstep(t);
    }
    if !animated {
        return 0.0;
    }
    if browsing_history || empty_state {
        return 1.0;
    }
    match turn_elapsed_ms {
        Some(elapsed) => smoothstep(elapsed as f32 / RAMP_MS as f32),
        None => 1.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OceanRamp {
    pub surface: Color,
    pub middle: Color,
    pub deep: Color,
    pub ambient: Color,
}

/// One continuous water column shared by every shell band in a frame.
///
/// Individual widgets still own their foreground and semantic surfaces, but
/// ordinary shell backgrounds sample this column with their absolute row.
/// That keeps the header, work strip, transcript, phase line, and composer
/// from each restarting the same miniature gradient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OceanColumn {
    ramp: OceanRamp,
    top: u16,
    height: u16,
    elapsed_ms: u128,
    completion_elapsed_ms: Option<u128>,
    phase: ShellPhase,
    animated: bool,
    /// Fixed-point (0..=1000) life presence; keeps `Eq` derivable.
    presence: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OceanRampCacheIdentity {
    ramp: OceanRamp,
    top: u16,
    height: u16,
    phase_tag: u8,
    animated: bool,
    completion_active: bool,
    presence: u16,
}

impl OceanRampCacheIdentity {
    fn fingerprint(self) -> u64 {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        [
            color_cache_code(self.ramp.surface),
            color_cache_code(self.ramp.middle),
            color_cache_code(self.ramp.deep),
            color_cache_code(self.ramp.ambient),
            u32::from(self.top),
            u32::from(self.height),
            u32::from(self.phase_tag),
            u32::from(self.animated),
            u32::from(self.completion_active),
            u32::from(self.presence),
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .fold(OFFSET_BASIS, |state, byte| {
            (state ^ u64::from(byte)).wrapping_mul(PRIME)
        })
    }
}

fn color_cache_code(value: Color) -> u32 {
    match value {
        Color::Reset => 0,
        Color::Black => 1,
        Color::Red => 2,
        Color::Green => 3,
        Color::Yellow => 4,
        Color::Blue => 5,
        Color::Magenta => 6,
        Color::Cyan => 7,
        Color::Gray => 8,
        Color::DarkGray => 9,
        Color::LightRed => 10,
        Color::LightGreen => 11,
        Color::LightYellow => 12,
        Color::LightBlue => 13,
        Color::LightMagenta => 14,
        Color::LightCyan => 15,
        Color::White => 16,
        Color::Indexed(index) => 0x0100_0000 | u32::from(index),
        Color::Rgb(red, green, blue) => 0x0200_0000 | u32::from_be_bytes([0, red, green, blue]),
    }
}

impl OceanColumn {
    #[must_use]
    pub fn new(
        ramp: OceanRamp,
        viewport: Rect,
        elapsed_ms: u128,
        completion_elapsed_ms: Option<u128>,
        phase: ShellPhase,
        animated: bool,
        presence: u16,
    ) -> Self {
        Self {
            ramp,
            top: viewport.y,
            height: viewport.height.max(1),
            elapsed_ms,
            completion_elapsed_ms,
            phase,
            animated,
            presence,
        }
    }

    #[must_use]
    pub fn color_at_y(self, y: u16) -> Color {
        let row = y.saturating_sub(self.top).min(self.height - 1);
        if let Some(elapsed) = self.completion_elapsed_ms {
            self.ramp.color_at_completion(row, self.height, elapsed)
        } else {
            // Ease between the static gradient and the phase treatment by
            // life presence, so mood/activity changes blend instead of snap.
            let static_color = self.ramp.color_at(row, self.height);
            if self.animated || self.presence > 0 {
                let phase_color =
                    self.ramp
                        .color_at_phase(row, self.height, self.elapsed_ms, self.phase);
                mix_colors(static_color, phase_color, self.presence_f32())
            } else {
                static_color
            }
        }
    }

    /// Life presence as a 0..=1 fraction of the fixed-point field.
    #[must_use]
    fn presence_f32(self) -> f32 {
        (f32::from(self.presence) / 1000.0).clamp(0.0, 1.0)
    }

    /// Elapsed milliseconds of the completion breath, when active. Ambient
    /// life uses this to time the rare whale cameo on successful turns.
    #[must_use]
    pub fn completion_elapsed_ms(self) -> Option<u128> {
        self.completion_elapsed_ms
    }

    /// Compact phase discriminator for [`crate::tui::ambient_life::OceanRampCache`].
    #[must_use]
    pub fn phase_tag(self) -> u8 {
        match self.phase {
            ShellPhase::Idle => 0,
            ShellPhase::Typing => 1,
            ShellPhase::Working => 2,
            ShellPhase::Verifying => 3,
            ShellPhase::Waiting => 4,
            ShellPhase::Approval => 5,
            ShellPhase::Done => 6,
            ShellPhase::Failed => 7,
        }
    }

    fn ramp_cache_identity(self) -> OceanRampCacheIdentity {
        OceanRampCacheIdentity {
            ramp: self.ramp,
            top: self.top,
            height: self.height,
            phase_tag: self.phase_tag(),
            animated: self.animated,
            completion_active: self.completion_elapsed_ms.is_some(),
            presence: self.presence,
        }
    }

    /// Deterministic fingerprint of every column input owned by the ramp cache.
    /// Actual colors are encoded explicitly; this never depends on randomized
    /// hashing or debug formatting.
    #[must_use]
    pub fn ramp_fingerprint(self) -> u64 {
        self.ramp_cache_identity().fingerprint()
    }

    #[must_use]
    pub fn with_viewport(mut self, viewport: Rect) -> Self {
        self.top = viewport.y;
        self.height = viewport.height.max(1);
        self
    }

    /// Continue the shared column through a shell-owned surface without
    /// flattening semantic highlights (selection, hover, error, code blocks).
    pub fn paint_matching(self, area: Rect, buf: &mut Buffer, background: Color) {
        for y in area.top()..area.bottom() {
            let row_bg = self.color_at_y(y);
            for x in area.left()..area.right() {
                let cell = &mut buf[(x, y)];
                if cell.bg == background {
                    cell.set_bg(row_bg);
                }
            }
        }
    }
}

impl OceanRamp {
    #[must_use]
    pub fn for_theme(theme: &UiTheme) -> Option<Self> {
        // Solarized Light's canonical Base3 (#fdf6e3) background is part of
        // the named palette's contract. Tinting it with the underwater field
        // turns the shell green-grey and no longer renders Solarized Light
        // (#4457). A non-canonical user-supplied background is a separate
        // contract and must keep the configured ombre treatment.
        if theme.mode == PaletteMode::SolarizedLight
            && theme.surface_bg == crate::palette::SOLARIZED_LIGHT_UI_THEME.surface_bg
        {
            return None;
        }

        // The canonical Whale pair gets the authored Codewhale water column.
        // Match both name and surface so a user-supplied `background_color`
        // remains the source of truth and still receives the generic ramp.
        if theme.name == crate::palette::UI_THEME.name
            && theme.surface_bg == crate::palette::UI_THEME.surface_bg
        {
            return Some(Self {
                surface: Color::Rgb(0x0e, 0x17, 0x29),
                middle: Color::Rgb(0x08, 0x11, 0x1c),
                deep: Color::Rgb(0x03, 0x07, 0x0d),
                ambient: Color::Rgb(0x26, 0x48, 0x66),
            });
        }
        if theme.name == crate::palette::LIGHT_UI_THEME.name
            && theme.surface_bg == crate::palette::LIGHT_UI_THEME.surface_bg
        {
            return Some(Self {
                surface: Color::Rgb(0xff, 0xfd, 0xf8),
                middle: Color::Rgb(0xf4, 0xf7, 0xfb),
                deep: Color::Rgb(0xf0, 0xf4, 0xf9),
                ambient: Color::Rgb(0x9a, 0xb8, 0xe0),
            });
        }

        let base = rgb(theme.surface_bg)?;
        let seafoam = rgb(theme.accent_secondary).unwrap_or((79, 209, 197));

        let (surface, middle, deep) = match theme.mode {
            PaletteMode::Light | PaletteMode::SolarizedLight => (
                mix(base, seafoam, 0.07),
                mix(base, seafoam, 0.13),
                mix(base, (70, 139, 196), 0.18),
            ),
            PaletteMode::Dark | PaletteMode::Grayscale => (
                mix(base, (30, 71, 103), 0.24),
                mix(base, (7, 30, 54), 0.40),
                mix(base, (2, 9, 24), 0.64),
            ),
        };

        Some(Self {
            surface: color(surface),
            middle: color(middle),
            deep: color(deep),
            ambient: color(mix(seafoam, base, 0.42)),
        })
    }

    #[must_use]
    pub fn color_at(self, row: u16, height: u16) -> Color {
        if height <= 1 {
            return self.surface;
        }
        let position = f32::from(row.min(height - 1)) / f32::from(height - 1);
        if position <= 0.42 {
            mix_colors(self.surface, self.middle, position / 0.42)
        } else {
            mix_colors(self.middle, self.deep, (position - 0.42) / 0.58)
        }
    }

    #[must_use]
    pub fn color_at_phase(
        self,
        row: u16,
        height: u16,
        elapsed_ms: u128,
        phase: ShellPhase,
    ) -> Color {
        let base = self.color_at(row, height);
        let depth = if height <= 1 {
            0.0
        } else {
            f32::from(row.min(height - 1)) / f32::from(height - 1)
        };
        if matches!(
            phase,
            ShellPhase::Waiting | ShellPhase::Approval | ShellPhase::Failed
        ) {
            return base;
        }
        let cycle = (elapsed_ms % 90_000) as f32 / 90_000.0;
        let breath = (cycle * std::f32::consts::TAU).sin() * 0.5 + 0.5;
        let (phase_bias, phase_depth) = match phase {
            ShellPhase::Idle => (0.035, 1.0 - depth),
            ShellPhase::Typing => (0.025, 1.0 - depth),
            ShellPhase::Working => (0.045, 0.35 + depth * 0.65),
            ShellPhase::Verifying => (0.055, 0.65 + (1.0 - depth) * 0.35),
            ShellPhase::Done => (0.018, 1.0 - depth),
            ShellPhase::Waiting | ShellPhase::Approval | ShellPhase::Failed => unreachable!(),
        };
        mix_colors(base, self.ambient, breath * phase_bias * phase_depth)
    }

    #[must_use]
    pub fn color_at_completion(self, row: u16, height: u16, elapsed_ms: u128) -> Color {
        let base = self.color_at(row, height);
        let elapsed = elapsed_ms.min(800) as f32 / 800.0;
        let brightness = if elapsed <= 0.4 {
            0.88 + (1.12 - 0.88) * (elapsed / 0.4)
        } else {
            1.12 + (1.0 - 1.12) * ((elapsed - 0.4) / 0.6)
        };
        scale_color(base, brightness)
    }
}

#[must_use]
fn rgb(value: Color) -> Option<(u8, u8, u8)> {
    match value {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        _ => None,
    }
}

#[must_use]
fn color((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

#[must_use]
pub fn mix_colors(from: Color, to: Color, amount: f32) -> Color {
    match (rgb(from), rgb(to)) {
        (Some(from), Some(to)) => color(mix(from, to, amount)),
        _ => from,
    }
}

#[must_use]
pub fn scale_color(value: Color, brightness: f32) -> Color {
    let Some((r, g, b)) = rgb(value) else {
        return value;
    };
    color((
        (f32::from(r) * brightness).round().clamp(0.0, 255.0) as u8,
        (f32::from(g) * brightness).round().clamp(0.0, 255.0) as u8,
        (f32::from(b) * brightness).round().clamp(0.0, 255.0) as u8,
    ))
}

#[must_use]
fn mix(from: (u8, u8, u8), to: (u8, u8, u8), amount: f32) -> (u8, u8, u8) {
    let amount = amount.clamp(0.0, 1.0);
    let channel = |a: u8, b: u8| {
        (f32::from(a) + (f32::from(b) - f32::from(a)) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    (
        channel(from.0, to.0),
        channel(from.1, to.1),
        channel(from.2, to.2),
    )
}

#[cfg(test)]
#[path = "ocean/tests.rs"]
mod tests;
