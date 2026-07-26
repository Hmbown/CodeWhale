//! Terminal palette-mode and color-depth detection.
//!
//! Detection returns *evidence*, not just a verdict: [`TerminalBackground`]
//! carries the background color we actually learned (when we learned one) and
//! [`BackgroundSource`] records how. The contrast floor in [`super::contrast`]
//! needs the color — a mode enum cannot tell you whether text clears 4.5:1 —
//! and the provenance is what lets us distinguish "measured dark" from
//! "assumed dark because nothing answered".

#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::OnceLock;

use ratatui::style::Color;

use super::contrast::relative_luminance;
use super::osc11;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Dark,
    Light,
    Grayscale,
    SolarizedLight,
}

/// How the terminal background was learned. Ordered strongest-first: an
/// answered OSC 11 query is a measurement, `COLORFGBG` is a hint, macOS
/// appearance is an inference about the OS rather than the terminal, and
/// `Unknown` means we are guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundSource {
    /// The terminal answered an OSC 11 query with its background color.
    Osc11,
    /// `COLORFGBG` was set. Carries a palette index, not an RGB value.
    ColorFgBg,
    /// macOS `AppleInterfaceStyle`. Describes the system, not the terminal —
    /// a dark-mode Mac can still be running a light-profile terminal.
    MacOsAppearance,
    /// No evidence at all. Callers must not treat this as "dark" for anything
    /// but choosing a default theme.
    Unknown,
}

/// What we know about the surface the TUI is drawing onto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalBackground {
    mode: PaletteMode,
    color: Option<Color>,
    source: BackgroundSource,
}

impl TerminalBackground {
    #[must_use]
    pub const fn new(mode: PaletteMode, color: Option<Color>, source: BackgroundSource) -> Self {
        Self {
            mode,
            color,
            source,
        }
    }

    /// The evidence-free default. Stays [`PaletteMode::Dark`] so terminals we
    /// cannot measure keep exactly the theme they get today, and carries no
    /// color so the contrast floor declines to act rather than acting on a
    /// guess.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::new(PaletteMode::Dark, None, BackgroundSource::Unknown)
    }

    #[must_use]
    pub const fn mode(&self) -> PaletteMode {
        self.mode
    }

    /// The measured background, or `None` when the source could not supply one
    /// (`COLORFGBG` indices 0–15, macOS appearance, no evidence).
    #[must_use]
    pub const fn color(&self) -> Option<Color> {
        self.color
    }

    #[must_use]
    pub const fn source(&self) -> BackgroundSource {
        self.source
    }
}

/// Luminance at which black text and white text have equal contrast against a
/// surface. Above it a surface is light; at or below it, dark. Derived from
/// the WCAG contrast formula: `(L+0.05)/0.05 == 1.05/(L+0.05)`.
const LIGHT_SURFACE_LUMINANCE: f32 = 0.179_129_5;

/// Classify a background color as light or dark by relative luminance. This is
/// the only place polarity is decided, so `#FFFFFF` and a pale ivory reach the
/// same verdict without anyone maintaining a list.
#[must_use]
pub fn palette_mode_for_background(color: Color) -> Option<PaletteMode> {
    let luminance = relative_luminance(color)?;
    Some(if luminance > LIGHT_SURFACE_LUMINANCE {
        PaletteMode::Light
    } else {
        PaletteMode::Dark
    })
}

impl PaletteMode {
    /// Parse `COLORFGBG`, whose last numeric segment is the terminal
    /// background color. Values >= 8 conventionally indicate a light profile.
    #[must_use]
    pub fn from_colorfgbg(value: &str) -> Option<Self> {
        let bg = colorfgbg_index(value)?;
        Some(if bg >= 8 { Self::Light } else { Self::Dark })
    }

    /// Detect the active palette mode. See [`terminal_background`] for the
    /// resolution order; this is the mode-only view of the same evidence.
    #[must_use]
    pub fn detect() -> Self {
        terminal_background().mode()
    }
}

/// The background segment of `COLORFGBG` — the last numeric field.
fn colorfgbg_index(value: &str) -> Option<u16> {
    value
        .split(';')
        .rev()
        .find_map(|part| part.parse::<u16>().ok())
}

/// Split `COLORFGBG` into a palette mode and, when the index is resolvable,
/// the background's actual RGB.
///
/// Indices 0–15 are remapped by the user's terminal profile, so we report the
/// mode without a color rather than inventing one. Indices >= 16 are fixed by
/// the xterm specification and can be resolved exactly.
fn colorfgbg_background(value: &str) -> Option<(PaletteMode, Option<Color>)> {
    let index = colorfgbg_index(value)?;
    if let Ok(index) = u8::try_from(index)
        && index >= 16
        && let Some(mode) = palette_mode_for_background(Color::Indexed(index))
    {
        return Some((mode, Some(Color::Indexed(index))));
    }
    Some((PaletteMode::from_colorfgbg(value)?, None))
}

/// Combine the available evidence into a single [`TerminalBackground`].
///
/// Pure, so every branch is testable without a terminal. Strongest evidence
/// wins: a measured color beats a palette index, which beats an OS appearance
/// setting, which beats nothing.
#[must_use]
pub fn resolve_terminal_background(
    osc11_rgb: Option<(u8, u8, u8)>,
    colorfgbg: Option<&str>,
    macos_fallback: Option<PaletteMode>,
) -> TerminalBackground {
    if let Some((r, g, b)) = osc11_rgb {
        let color = Color::Rgb(r, g, b);
        if let Some(mode) = palette_mode_for_background(color) {
            return TerminalBackground::new(mode, Some(color), BackgroundSource::Osc11);
        }
    }
    if let Some((mode, color)) = colorfgbg.and_then(colorfgbg_background) {
        return TerminalBackground::new(mode, color, BackgroundSource::ColorFgBg);
    }
    if let Some(mode) = macos_fallback {
        return TerminalBackground::new(mode, None, BackgroundSource::MacOsAppearance);
    }
    TerminalBackground::unknown()
}

static TERMINAL_BACKGROUND: OnceLock<TerminalBackground> = OnceLock::new();

/// The detected terminal background, without querying the terminal.
///
/// Returns the probed result once [`probe_terminal_background`] has run;
/// before that it answers from the environment alone. It deliberately does not
/// populate the cache, so an early caller cannot lock in an env-only answer
/// that the probe would have improved.
#[must_use]
pub fn terminal_background() -> TerminalBackground {
    if let Some(background) = TERMINAL_BACKGROUND.get() {
        return *background;
    }
    resolve_terminal_background(
        None,
        std::env::var("COLORFGBG").ok().as_deref(),
        detect_macos_palette_mode(),
    )
}

/// Query the terminal for its background and cache the result.
///
/// Call this once, from the TUI entry point, after raw mode is enabled and
/// before the event loop starts — see the caveat on
/// [`osc11::query_terminal_background`]. Safe to call more than once; only the
/// first result is kept, so the answer stays stable for the process.
pub fn probe_terminal_background() -> TerminalBackground {
    if let Some(background) = TERMINAL_BACKGROUND.get() {
        return *background;
    }
    let background = resolve_terminal_background(
        osc11::query_terminal_background(osc11::OSC11_QUERY_TIMEOUT),
        std::env::var("COLORFGBG").ok().as_deref(),
        detect_macos_palette_mode(),
    );
    *TERMINAL_BACKGROUND.get_or_init(|| background)
}

#[cfg(target_os = "macos")]
fn detect_macos_palette_mode() -> Option<PaletteMode> {
    let output = Command::new("defaults")
        .args(["read", "-g", "AppleInterfaceStyle"])
        .output()
        .ok()?;

    if output.status.success() {
        Some(palette_mode_from_apple_interface_style(
            &String::from_utf8_lossy(&output.stdout),
        ))
    } else {
        Some(PaletteMode::Light)
    }
}

#[cfg(not(target_os = "macos"))]
fn detect_macos_palette_mode() -> Option<PaletteMode> {
    None
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn palette_mode_from_apple_interface_style(value: &str) -> PaletteMode {
    if value.trim().eq_ignore_ascii_case("dark") {
        PaletteMode::Dark
    } else {
        PaletteMode::Light
    }
}
