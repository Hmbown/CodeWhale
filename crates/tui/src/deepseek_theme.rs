//! Codewhale terminal theme tokens (legacy module path).
//!
//! A small, deliberately flat module that names the color, border, and
//! padding choices the TUI is making. Values follow the semantic grammar
//! exposed by [`crate::palette`], keeping the older module path for source
//! compatibility.
//!
//! The only consumers today are tool cell renderers in [`crate::tui::history`]
//! and sidebar section chrome in [`crate::tui::ui`]. All other call sites
//! continue to use [`crate::palette`] directly until they are migrated.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{BorderType, Borders, Padding};

use crate::palette;
use crate::palette::PaletteMode;
use crate::tui::history::ToolStatus;

/// Visual variant exposed by the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Dark,
    Light,
    Grayscale,
}

/// Centralized visual tokens for sidebar and tool rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub variant: Variant,

    // Sidebar / section chrome
    pub section_borders: Borders,
    pub section_border_type: BorderType,
    pub section_border_color: Color,
    pub section_bg: Color,
    pub section_title_color: Color,
    pub section_padding: Padding,

    // Tool cell color tokens
    pub tool_title_color: Color,
    pub tool_value_color: Color,
    pub tool_label_color: Color,
    pub tool_running_accent: Color,
    pub tool_success_accent: Color,
    pub tool_warning_accent: Color,
    pub tool_failed_accent: Color,
}

impl Theme {
    /// The current dark theme. Visible output today uses these values.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            variant: Variant::Dark,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::BORDER_COLOR,
            section_bg: palette::WHALE_BG,
            section_title_color: palette::WHALE_ACTION,
            // Horizontal padding only. `Padding::uniform(1)` ate two rows of
            // each sidebar panel — for compact terminals where Work/Tasks/Agents
            // get ~3 rows total via the 25% layout split, that left zero rows
            // for content (#63 follow-up: panels rendered as empty boxes even
            // when "No todos" / "No active plan" should have shown).
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::TEXT_SOFT,
            tool_value_color: palette::TEXT_MUTED,
            tool_label_color: palette::TEXT_DIM,
            tool_running_accent: palette::WHALE_ACTION,
            tool_success_accent: palette::TEXT_MUTED,
            tool_warning_accent: palette::WHALE_HUMAN,
            tool_failed_accent: palette::WHALE_ERROR,
        }
    }

    /// Light theme tokens for sidebar and tool chrome.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            variant: Variant::Light,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::LIGHT_BORDER,
            section_bg: palette::LIGHT_PANEL,
            section_title_color: palette::LIGHT_ACTION,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::LIGHT_TEXT_SOFT,
            tool_value_color: palette::LIGHT_TEXT_MUTED,
            tool_label_color: palette::LIGHT_TEXT_HINT,
            tool_running_accent: palette::LIGHT_ACTION,
            tool_success_accent: palette::LIGHT_TEXT_MUTED,
            tool_warning_accent: palette::LIGHT_WARNING,
            tool_failed_accent: palette::LIGHT_DANGER,
        }
    }

    /// Solarized Light theme tokens — warm ivory tones, high contrast.
    #[must_use]
    pub const fn solarized_light() -> Self {
        Self {
            variant: Variant::Light,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::SOLARIZED_BORDER,
            section_bg: palette::SOLARIZED_PANEL,
            section_title_color: palette::SOLARIZED_BLUE,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::SOLARIZED_TEXT_SOFT,
            tool_value_color: palette::SOLARIZED_TEXT_MUTED,
            tool_label_color: palette::SOLARIZED_TEXT_DIM,
            tool_running_accent: palette::SOLARIZED_BLUE,
            tool_success_accent: palette::SOLARIZED_TEXT_MUTED,
            tool_warning_accent: palette::SOLARIZED_YELLOW,
            tool_failed_accent: palette::SOLARIZED_RED,
        }
    }

    /// Neutral black/white tokens for users who want minimal brand color.
    #[must_use]
    pub const fn grayscale() -> Self {
        Self {
            variant: Variant::Grayscale,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::GRAYSCALE_BORDER,
            section_bg: palette::GRAYSCALE_PANEL,
            section_title_color: palette::GRAYSCALE_TEXT_SOFT,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::GRAYSCALE_TEXT_SOFT,
            tool_value_color: palette::GRAYSCALE_TEXT_MUTED,
            tool_label_color: palette::GRAYSCALE_TEXT_HINT,
            tool_running_accent: palette::GRAYSCALE_TEXT_SOFT,
            tool_success_accent: palette::GRAYSCALE_TEXT_HINT,
            tool_warning_accent: palette::GRAYSCALE_TEXT_MUTED,
            tool_failed_accent: palette::GRAYSCALE_TEXT_BODY,
        }
    }

    #[must_use]
    pub const fn for_palette_mode(mode: PaletteMode) -> Self {
        match mode {
            PaletteMode::Dark => Self::dark(),
            PaletteMode::Light => Self::light(),
            PaletteMode::Grayscale => Self::grayscale(),
            PaletteMode::SolarizedLight => Self::solarized_light(),
        }
    }

    /// The one place a tool cell's lifecycle state becomes ink.
    ///
    /// Every surface that paints a tool cell — the header glyph, the family
    /// glyph, the state word, the card rail, and the exploring fan-out dots —
    /// reads its colour from here, so a running tool reads as running and a
    /// failed one as failed without any neighbouring row narrating it.
    /// Modelled on OMP's `output-block.ts`, where a block's `state` drives its
    /// border colour: in-flight takes the action accent, a settled success
    /// recedes into muted text, and only warning and failure keep a loud
    /// colour of their own. `Hydrated` is a stalled "tool loaded — retry
    /// required", not live work, so it takes the hint colour rather than
    /// borrowing the running accent and reading as in-flight.
    #[must_use]
    pub const fn tool_status_color(self, status: ToolStatus) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            ToolStatus::Success => self.tool_success_accent,
            ToolStatus::Hydrated => self.tool_label_color,
            ToolStatus::Warning => self.tool_warning_accent,
            ToolStatus::Failed => self.tool_failed_accent,
        }
    }

    /// Bold tool title style (e.g. "Plan", "Shell").
    #[must_use]
    pub fn tool_title_style(self) -> Style {
        Style::default()
            .fg(self.tool_title_color)
            .add_modifier(Modifier::BOLD)
    }

    /// Right-side status text ("running", "done", "issue") style.
    #[must_use]
    pub fn tool_status_style(self, status: ToolStatus) -> Style {
        Style::default().fg(self.tool_status_color(status))
    }

    /// Detail label style ("command:", "time:", step markers).
    #[must_use]
    pub fn tool_label_style(self) -> Style {
        Style::default().fg(self.tool_label_color)
    }

    /// Default value style for tool detail rows.
    #[must_use]
    pub fn tool_value_style(self) -> Style {
        Style::default().fg(self.tool_value_color)
    }
}

/// Returns the active theme used by the TUI today.
#[must_use]
pub const fn active_theme() -> Theme {
    Theme::dark()
}

#[cfg(test)]
mod tests {
    use super::{Theme, Variant, active_theme};
    use crate::palette;
    use crate::tui::history::ToolStatus;

    #[test]
    fn active_theme_returns_dark() {
        assert_eq!(active_theme(), Theme::dark());
    }

    #[test]
    fn dark_theme_uses_codewhale_semantic_roles() {
        let theme = Theme::dark();
        assert_eq!(theme.variant, Variant::Dark);
        assert_eq!(theme.section_border_color, palette::BORDER_COLOR);
        assert_eq!(theme.section_bg, palette::WHALE_BG);
        assert_eq!(theme.section_title_color, palette::WHALE_ACTION);
        assert_eq!(theme.tool_title_color, palette::TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::TEXT_MUTED);
        assert_eq!(theme.tool_label_color, palette::TEXT_DIM);
        assert_eq!(theme.tool_running_accent, palette::WHALE_ACTION);
        assert_eq!(theme.tool_success_accent, palette::TEXT_MUTED);
        assert_eq!(theme.tool_failed_accent, palette::WHALE_ERROR);
    }

    #[test]
    fn light_theme_uses_light_panel_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Light);
        assert_eq!(theme.variant, Variant::Light);
        assert_eq!(theme.section_bg, palette::LIGHT_PANEL);
        assert_eq!(theme.section_border_color, palette::LIGHT_BORDER);
        assert_eq!(theme.tool_title_color, palette::LIGHT_TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::LIGHT_TEXT_MUTED);
        assert_eq!(theme.section_title_color, palette::LIGHT_ACTION);
        assert_eq!(theme.tool_running_accent, palette::LIGHT_ACTION);
        assert_eq!(theme.tool_success_accent, palette::LIGHT_TEXT_MUTED);
    }

    #[test]
    fn grayscale_theme_uses_neutral_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Grayscale);
        assert_eq!(theme.variant, Variant::Grayscale);
        assert_eq!(theme.section_bg, palette::GRAYSCALE_PANEL);
        assert_eq!(theme.section_border_color, palette::GRAYSCALE_BORDER);
        assert_eq!(theme.tool_running_accent, palette::GRAYSCALE_TEXT_SOFT);
        assert_eq!(theme.tool_failed_accent, palette::GRAYSCALE_TEXT_BODY);
    }

    /// The whole point of "tool cells carry their own state": every variant
    /// maps to a distinct WHALE token, so a running card, a failed one and a
    /// settled one are told apart by ink alone. Asserted against the tokens
    /// rather than the theme's own fields — a field-to-field assertion passes
    /// no matter what the fields hold, which is how `Hydrated` sat on the
    /// running accent and read as live work.
    #[test]
    fn tool_status_color_maps_each_status() {
        let theme = Theme::dark();
        let table = [
            (ToolStatus::Running, palette::WHALE_ACTION),
            (ToolStatus::Success, palette::TEXT_MUTED),
            (ToolStatus::Hydrated, palette::TEXT_DIM),
            (ToolStatus::Warning, palette::WHALE_HUMAN),
            (ToolStatus::Failed, palette::WHALE_ERROR),
        ];
        for (status, expected) in table {
            assert_eq!(
                theme.tool_status_color(status),
                expected,
                "dark theme paints {status:?} with the wrong token"
            );
        }

        // Nothing in the table may collide: two statuses sharing a colour is
        // the failure this mapping exists to prevent.
        for (i, (status, color)) in table.iter().enumerate() {
            for (other_status, other_color) in &table[i + 1..] {
                assert_ne!(
                    color, other_color,
                    "{status:?} and {other_status:?} paint the same colour"
                );
            }
        }
    }

    /// The load-bearing separations hold in every palette, not just the active
    /// one. Grayscale has four greys for five statuses, so full distinctness is
    /// a dark-theme claim; running / failed / success being told apart is not
    /// negotiable anywhere.
    #[test]
    fn every_palette_separates_running_failed_and_success() {
        for mode in [
            crate::palette::PaletteMode::Dark,
            crate::palette::PaletteMode::Light,
            crate::palette::PaletteMode::Grayscale,
            crate::palette::PaletteMode::SolarizedLight,
        ] {
            let theme = Theme::for_palette_mode(mode);
            assert_ne!(
                theme.tool_status_color(ToolStatus::Running),
                theme.tool_status_color(ToolStatus::Failed),
                "{mode:?} paints running and failed alike"
            );
            assert_ne!(
                theme.tool_status_color(ToolStatus::Running),
                theme.tool_status_color(ToolStatus::Success),
                "{mode:?} paints running and success alike"
            );
            assert_ne!(
                theme.tool_status_color(ToolStatus::Failed),
                theme.tool_status_color(ToolStatus::Success),
                "{mode:?} paints failed and success alike"
            );
        }
    }
}
