//! Whale/DeepSeek terminal theme tokens.
//!
//! A small, deliberately flat module that names the color, border, and
//! padding choices the TUI is already making. All values match the dark
//! palette previously hard-coded against [`crate::palette`]; a single
//! source-of-truth change here can swap the skin later. Visible output
//! is not changed by introducing this module.
//!
//! The only consumers today are the plan and tool cell renderers in
//! [`crate::tui::history`] and the sidebar section chrome in
//! [`crate::tui::ui`]. All other call sites continue to use [`crate::palette`]
//! directly until they are migrated in a later slice.

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

/// Centralized visual tokens for sidebar, plan, and tool rendering.
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
    pub tool_failed_accent: Color,

    // Plan cell color tokens
    pub plan_progress_color: Color,
    pub plan_summary_color: Color,
    pub plan_explanation_color: Color,
    pub plan_pending_color: Color,
    pub plan_in_progress_color: Color,
    pub plan_completed_color: Color,
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
            section_bg: palette::DEEPSEEK_INK,
            section_title_color: palette::WHALE_ACCENT_PRIMARY,
            // Horizontal padding only. `Padding::uniform(1)` ate two rows of
            // each sidebar panel — for compact terminals where Work/Tasks/Agents
            // get ~3 rows total via the 25% layout split, that left zero rows
            // for content (#63 follow-up: panels rendered as empty boxes even
            // when "No todos" / "No active plan" should have shown).
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::TEXT_SOFT,
            tool_value_color: palette::TEXT_MUTED,
            tool_label_color: palette::TEXT_DIM,
            tool_running_accent: palette::ACCENT_TOOL_LIVE,
            tool_success_accent: palette::TEXT_DIM,
            tool_failed_accent: palette::ACCENT_TOOL_ISSUE,
            plan_progress_color: palette::STATUS_SUCCESS,
            plan_summary_color: palette::TEXT_MUTED,
            plan_explanation_color: palette::TEXT_DIM,
            plan_pending_color: palette::TEXT_MUTED,
            plan_in_progress_color: palette::STATUS_WARNING,
            plan_completed_color: palette::STATUS_SUCCESS,
        }
    }

    /// Light theme tokens for sidebar and tool chrome. Accents use the
    /// light-mode tokens (blue / amber / deep red) — the dark palette's
    /// Signal Gold and Rose Red don't clear readable contrast on light
    /// surfaces.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            variant: Variant::Light,
            section_borders: Borders::ALL,
            section_border_type: BorderType::Plain,
            section_border_color: palette::LIGHT_BORDER,
            section_bg: palette::LIGHT_PANEL,
            section_title_color: palette::LIGHT_ACCENT_BLUE,
            section_padding: Padding::horizontal(1),
            tool_title_color: palette::LIGHT_TEXT_SOFT,
            tool_value_color: palette::LIGHT_TEXT_MUTED,
            tool_label_color: palette::LIGHT_TEXT_HINT,
            tool_running_accent: palette::LIGHT_ACCENT_BLUE,
            tool_success_accent: palette::LIGHT_TEXT_HINT,
            tool_failed_accent: palette::LIGHT_ERROR_FG,
            plan_progress_color: palette::LIGHT_GREEN,
            plan_summary_color: palette::LIGHT_TEXT_MUTED,
            plan_explanation_color: palette::LIGHT_TEXT_HINT,
            plan_pending_color: palette::LIGHT_TEXT_MUTED,
            plan_in_progress_color: palette::LIGHT_AMBER,
            plan_completed_color: palette::LIGHT_GREEN,
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
            tool_failed_accent: palette::SOLARIZED_RED,
            plan_progress_color: palette::SOLARIZED_BLUE,
            plan_summary_color: palette::SOLARIZED_TEXT_MUTED,
            plan_explanation_color: palette::SOLARIZED_TEXT_DIM,
            plan_pending_color: palette::SOLARIZED_TEXT_MUTED,
            plan_in_progress_color: palette::SOLARIZED_ORANGE,
            plan_completed_color: palette::SOLARIZED_BLUE,
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
            tool_failed_accent: palette::GRAYSCALE_TEXT_BODY,
            plan_progress_color: palette::GRAYSCALE_TEXT_SOFT,
            plan_summary_color: palette::GRAYSCALE_TEXT_MUTED,
            plan_explanation_color: palette::GRAYSCALE_TEXT_HINT,
            plan_pending_color: palette::GRAYSCALE_TEXT_MUTED,
            plan_in_progress_color: palette::GRAYSCALE_TEXT_BODY,
            plan_completed_color: palette::GRAYSCALE_TEXT_SOFT,
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

    /// Pick the right tool accent for a given [`ToolStatus`].
    #[must_use]
    pub const fn tool_status_color(self, status: ToolStatus) -> Color {
        match status {
            ToolStatus::Running => self.tool_running_accent,
            ToolStatus::Success => self.tool_success_accent,
            ToolStatus::Hydrated => self.tool_running_accent,
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

// The render loop publishes the active `PaletteMode` here (via
// `ColorCompatBackend::set_palette_mode`, called once per frame from the
// draw path) so free-function style helpers — the tool/plan card styling in
// `crate::tui::history` — can resolve theme-correct tokens without threading
// `App` through every call site. Rendering is single-threaded, and a
// `thread_local` (rather than a process-global) keeps the `#[test]`-per-thread
// harness isolated: a test that flips the mode can't bleed into tests
// asserting dark-theme styles on other threads.
thread_local! {
    static ACTIVE_PALETTE_MODE: std::cell::Cell<PaletteMode> =
        const { std::cell::Cell::new(PaletteMode::Dark) };
}

/// Publish the palette mode the current thread should style against.
pub fn set_active_palette_mode(mode: PaletteMode) {
    ACTIVE_PALETTE_MODE.with(|cell| cell.set(mode));
}

/// The palette mode last published on this thread (dark until told otherwise).
#[must_use]
pub fn active_palette_mode() -> PaletteMode {
    ACTIVE_PALETTE_MODE.with(std::cell::Cell::get)
}

/// Returns the active theme used by the TUI: the token set matching the
/// palette mode the render loop last published. Defaults to dark, which is
/// also correct for the community presets (Catppuccin, Tokyo Night, …) —
/// those report `PaletteMode::Dark` and restyle dark tokens per-cell in the
/// backend remap.
#[must_use]
pub fn active_theme() -> Theme {
    Theme::for_palette_mode(active_palette_mode())
}

#[cfg(test)]
mod tests {
    use super::{Theme, Variant, active_theme};
    use crate::palette;
    use crate::tui::history::ToolStatus;

    #[test]
    fn active_theme_defaults_to_dark() {
        // Fresh thread ⇒ nothing published yet ⇒ dark tokens. Run on a
        // dedicated thread so this can't observe a mode set by another test
        // sharing this thread in single-threaded test runs.
        std::thread::spawn(|| assert_eq!(active_theme(), Theme::dark()))
            .join()
            .expect("thread");
    }

    #[test]
    fn active_theme_follows_published_palette_mode() {
        // thread_local storage keeps this from bleeding into other tests,
        // but reset to Dark on the way out anyway for single-threaded runs.
        use crate::palette::PaletteMode;
        super::set_active_palette_mode(PaletteMode::Light);
        assert_eq!(active_theme(), Theme::light());
        super::set_active_palette_mode(PaletteMode::SolarizedLight);
        assert_eq!(active_theme(), Theme::solarized_light());
        super::set_active_palette_mode(PaletteMode::Grayscale);
        assert_eq!(active_theme(), Theme::grayscale());
        super::set_active_palette_mode(PaletteMode::Dark);
        assert_eq!(active_theme(), Theme::dark());
    }

    #[test]
    fn dark_theme_matches_existing_palette_choices() {
        let theme = Theme::dark();
        assert_eq!(theme.variant, Variant::Dark);
        assert_eq!(theme.section_border_color, palette::BORDER_COLOR);
        assert_eq!(theme.section_bg, palette::DEEPSEEK_INK);
        assert_eq!(theme.section_title_color, palette::WHALE_ACCENT_PRIMARY);
        assert_eq!(theme.tool_title_color, palette::TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::TEXT_MUTED);
        assert_eq!(theme.tool_label_color, palette::TEXT_DIM);
        assert_eq!(theme.tool_running_accent, palette::ACCENT_TOOL_LIVE);
        assert_eq!(theme.tool_success_accent, palette::TEXT_DIM);
        assert_eq!(theme.tool_failed_accent, palette::ACCENT_TOOL_ISSUE);
    }

    #[test]
    fn light_theme_uses_light_panel_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Light);
        assert_eq!(theme.variant, Variant::Light);
        assert_eq!(theme.section_bg, palette::LIGHT_PANEL);
        assert_eq!(theme.section_border_color, palette::LIGHT_BORDER);
        assert_eq!(theme.tool_title_color, palette::LIGHT_TEXT_SOFT);
        assert_eq!(theme.tool_value_color, palette::LIGHT_TEXT_MUTED);
        assert_eq!(theme.plan_summary_color, palette::LIGHT_TEXT_MUTED);
    }

    #[test]
    fn light_theme_accents_avoid_dark_palette_gold_and_rose() {
        // Signal Gold (#F6C453) and Rose Red (#FF5C7A) are dark-surface
        // accents; on light panels they fall to ~1.5:1 / ~2.8:1. The light
        // token set must not reach for them.
        let theme = Theme::light();
        for accent in [
            theme.section_title_color,
            theme.tool_running_accent,
            theme.tool_failed_accent,
            theme.plan_progress_color,
            theme.plan_in_progress_color,
            theme.plan_completed_color,
        ] {
            assert_ne!(accent, palette::WHALE_ACCENT_PRIMARY);
            assert_ne!(accent, palette::DEEPSEEK_RED);
        }
        assert_eq!(theme.tool_running_accent, palette::LIGHT_ACCENT_BLUE);
        assert_eq!(theme.tool_failed_accent, palette::LIGHT_ERROR_FG);
        assert_eq!(theme.plan_in_progress_color, palette::LIGHT_AMBER);
    }

    #[test]
    fn grayscale_theme_uses_neutral_tokens() {
        let theme = Theme::for_palette_mode(crate::palette::PaletteMode::Grayscale);
        assert_eq!(theme.variant, Variant::Grayscale);
        assert_eq!(theme.section_bg, palette::GRAYSCALE_PANEL);
        assert_eq!(theme.section_border_color, palette::GRAYSCALE_BORDER);
        assert_eq!(theme.tool_running_accent, palette::GRAYSCALE_TEXT_SOFT);
        assert_eq!(theme.tool_failed_accent, palette::GRAYSCALE_TEXT_BODY);
        assert_eq!(theme.plan_summary_color, palette::GRAYSCALE_TEXT_MUTED);
    }

    #[test]
    fn tool_status_color_maps_each_status() {
        let theme = Theme::dark();
        assert_eq!(
            theme.tool_status_color(ToolStatus::Running),
            theme.tool_running_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Success),
            theme.tool_success_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Hydrated),
            theme.tool_running_accent
        );
        assert_eq!(
            theme.tool_status_color(ToolStatus::Failed),
            theme.tool_failed_accent
        );
    }
}
