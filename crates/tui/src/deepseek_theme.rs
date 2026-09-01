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
use crate::tui::widgets::tool_card::ToolFamily;

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
            tool_success_accent: palette::STATUS_SUCCESS,
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
            tool_success_accent: palette::LIGHT_SUCCESS_FG,
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
            tool_success_accent: palette::SOLARIZED_CYAN,
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

    /// Colour of a tool cell's **rail** — the card border.
    ///
    /// This is OMP's `output-block.ts` rule verbatim: a block takes a state and
    /// its border colour follows it. In-flight takes the action accent, a
    /// settled success recedes into muted text so finished work stops competing
    /// for the eye, and only warning and failure keep a loud colour. `Hydrated`
    /// is a stalled "tool loaded — retry required", not live work, so it takes
    /// the hint colour instead of borrowing the running accent and reading as
    /// in-flight.
    ///
    /// Deliberately *not* the same function as [`Self::tool_glyph_color`]: the
    /// border reports lifecycle, the glyph reports identity. They agree
    /// wherever it matters — running, warning and failure are the same ink in
    /// both, so the two can never disagree about trouble.
    #[must_use]
    pub const fn tool_rail_color(self, status: ToolStatus) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            ToolStatus::Success => self.tool_value_color,
            ToolStatus::Hydrated => self.tool_label_color,
            ToolStatus::Warning => self.tool_warning_accent,
            ToolStatus::Failed => self.tool_failed_accent,
        }
    }

    /// Colour of a tool cell's **status glyph** and the state word beside it.
    ///
    /// Follows the accepted mockup (`tideline-mockups/tideline-01`) rather than
    /// the border rule: a finished verify row keeps its green `✓`, and a
    /// finished read or search keeps the family accent that identifies it — the
    /// blue magnifier in that mockup. A settled card therefore still says *what
    /// it was* even while its border has receded to muted.
    ///
    /// Everything that needs attention reads identically to the rail.
    #[must_use]
    pub const fn tool_glyph_color(self, status: ToolStatus, family: ToolFamily) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            // Verified work earns Working Green; every other family keeps the
            // action accent it wore while running, which is what makes a
            // completed `read` row still read as a read.
            ToolStatus::Success => match family {
                ToolFamily::Verify => self.tool_success_accent,
                _ => self.tool_running_accent,
            },
            // A hydrated cell has not succeeded at anything yet, so it never
            // borrows the verified or family accent.
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

    /// Right-side status text ("running", "done", "issue") style. Reads as the
    /// glyph it sits beside, not as the rail.
    #[must_use]
    pub fn tool_status_style(self, status: ToolStatus, family: ToolFamily) -> Style {
        Style::default().fg(self.tool_glyph_color(status, family))
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
    use crate::tui::widgets::tool_card::ToolFamily;

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
        assert_eq!(theme.tool_success_accent, palette::STATUS_SUCCESS);
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
        assert_eq!(theme.tool_success_accent, palette::LIGHT_SUCCESS_FG);
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

    /// The rail is the block's border and follows OMP's rule: every status is a
    /// distinct WHALE token and a settled success recedes into muted text.
    /// Asserted against the tokens rather than the theme's own fields — a
    /// field-to-field assertion passes no matter what the fields hold, which is
    /// how `Hydrated` sat on the running accent and read as live work.
    #[test]
    fn tool_rail_color_maps_each_status() {
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
                theme.tool_rail_color(status),
                expected,
                "dark theme borders {status:?} with the wrong token"
            );
        }

        // Nothing in the table may collide: two statuses sharing a border
        // colour is the failure this mapping exists to prevent.
        for (i, (status, color)) in table.iter().enumerate() {
            for (other_status, other_color) in &table[i + 1..] {
                assert_ne!(
                    color, other_color,
                    "{status:?} and {other_status:?} draw the same rail"
                );
            }
        }
    }

    /// The glyph follows the accepted mockup, not the border rule: a settled
    /// verify row keeps Working Green, a settled read keeps the family accent
    /// that identifies it, and everything needing attention reads exactly as
    /// the rail does.
    #[test]
    fn tool_glyph_color_maps_each_status_and_family() {
        let theme = Theme::dark();
        // `STATUS_SUCCESS` is built from `WHALE_WORKING_GREEN_RGB`.
        let working_green = palette::STATUS_SUCCESS;
        let table = [
            (ToolStatus::Running, ToolFamily::Read, palette::WHALE_ACTION),
            (
                ToolStatus::Running,
                ToolFamily::Verify,
                palette::WHALE_ACTION,
            ),
            (ToolStatus::Success, ToolFamily::Verify, working_green),
            (ToolStatus::Success, ToolFamily::Read, palette::WHALE_ACTION),
            (ToolStatus::Success, ToolFamily::Find, palette::WHALE_ACTION),
            (ToolStatus::Success, ToolFamily::Run, palette::WHALE_ACTION),
            (
                ToolStatus::Success,
                ToolFamily::Generic,
                palette::WHALE_ACTION,
            ),
            (ToolStatus::Hydrated, ToolFamily::Verify, palette::TEXT_DIM),
            (ToolStatus::Hydrated, ToolFamily::Read, palette::TEXT_DIM),
            (ToolStatus::Warning, ToolFamily::Read, palette::WHALE_HUMAN),
            (ToolStatus::Failed, ToolFamily::Verify, palette::WHALE_ERROR),
            (ToolStatus::Failed, ToolFamily::Read, palette::WHALE_ERROR),
        ];
        for (status, family, expected) in table {
            assert_eq!(
                theme.tool_glyph_color(status, family),
                expected,
                "dark theme paints the {status:?}/{family:?} glyph with the wrong token"
            );
        }
    }

    /// The two mappings are separate on purpose, and each half of that claim is
    /// load-bearing: a settled card must dim its border while keeping an
    /// identifying glyph, and the two must never disagree about trouble.
    #[test]
    fn rail_and_glyph_split_only_where_the_card_has_settled() {
        let theme = Theme::dark();
        for family in [ToolFamily::Read, ToolFamily::Verify] {
            for status in [ToolStatus::Running, ToolStatus::Warning, ToolStatus::Failed] {
                assert_eq!(
                    theme.tool_rail_color(status),
                    theme.tool_glyph_color(status, family),
                    "{status:?} must read the same on the rail and the glyph"
                );
            }
            assert_ne!(
                theme.tool_rail_color(ToolStatus::Success),
                theme.tool_glyph_color(ToolStatus::Success, family),
                "a settled {family:?} card must dim its border without dimming its glyph"
            );
        }
        assert_ne!(
            theme.tool_glyph_color(ToolStatus::Success, ToolFamily::Verify),
            theme.tool_glyph_color(ToolStatus::Success, ToolFamily::Read),
            "a passed verify and a finished read must not share a glyph colour"
        );
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
                theme.tool_rail_color(ToolStatus::Running),
                theme.tool_rail_color(ToolStatus::Failed),
                "{mode:?} paints running and failed alike"
            );
            assert_ne!(
                theme.tool_rail_color(ToolStatus::Running),
                theme.tool_rail_color(ToolStatus::Success),
                "{mode:?} paints running and success alike"
            );
            assert_ne!(
                theme.tool_rail_color(ToolStatus::Failed),
                theme.tool_rail_color(ToolStatus::Success),
                "{mode:?} paints failed and success alike"
            );
        }
    }
}
