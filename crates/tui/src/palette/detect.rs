//! Terminal palette-mode and color-depth detection.

#[cfg(target_os = "macos")]
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Dark,
    Light,
    Grayscale,
    SolarizedLight,
}

impl PaletteMode {
    /// Parse `COLORFGBG`, whose last numeric segment is the terminal
    /// background color. Values >= 8 conventionally indicate a light profile.
    #[must_use]
    pub fn from_colorfgbg(value: &str) -> Option<Self> {
        let bg = value
            .split(';')
            .rev()
            .find_map(|part| part.parse::<u16>().ok())?;
        Some(if bg >= 8 { Self::Light } else { Self::Dark })
    }

    /// Detect the active palette mode. `COLORFGBG` wins when present; macOS
    /// appearance is a fallback for terminals that omit terminal color hints.
    /// Missing or unparsable values default to dark so existing terminal setups
    /// keep the tuned theme.
    #[must_use]
    pub fn detect() -> Self {
        Self::detect_from_sources(
            std::env::var("COLORFGBG").ok().as_deref(),
            detect_macos_palette_mode(),
        )
    }

    #[must_use]
    pub(crate) fn detect_from_sources(
        colorfgbg: Option<&str>,
        macos_fallback: Option<Self>,
    ) -> Self {
        colorfgbg
            .and_then(Self::from_colorfgbg)
            .or(macos_fallback)
            .unwrap_or(Self::Dark)
    }
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
