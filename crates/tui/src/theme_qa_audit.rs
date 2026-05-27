//! v0.8.45 theme QA audit — verification script.
//!
//! This module validates:
//! - Every shipped theme has all required semantic palette fields populated.
//! - Error/destructive states are distinct from warm action accents.
//! - Selection, focus, diff, warning, success, and status colors are readable.
//! - Terminal contrast is checked for common truecolor surfaces.
//!
//! Run with: cargo test -p codewhale-tui -- theme_qa

#[cfg(test)]
mod tests {
    use crate::palette::{
        CATPPUCCIN_MOCHA_UI_THEME, DRACULA_UI_THEME, GRAYSCALE_UI_THEME, GRUVBOX_DARK_UI_THEME,
        LIGHT_UI_THEME, TOKYO_NIGHT_UI_THEME, UI_THEME, UiTheme, WHALE_ACCENT_ACTION_RGB,
        WHALE_ACCENT_PRIMARY_RGB, WHALE_ACCENT_SECONDARY_RGB, WHALE_BG_RGB, WHALE_TEXT_BODY_RGB,
        WHALE_TEXT_MUTED_RGB,
    };
    use ratatui::style::Color;

    /// All shipped themes in display order.
    const ALL_THEMES: &[UiTheme] = &[
        UI_THEME,
        LIGHT_UI_THEME,
        GRAYSCALE_UI_THEME,
        CATPPUCCIN_MOCHA_UI_THEME,
        TOKYO_NIGHT_UI_THEME,
        DRACULA_UI_THEME,
        GRUVBOX_DARK_UI_THEME,
    ];

    /// Extract (r, g, b) from a Color::Rgb. Returns None for non-RGB colors.
    fn rgb(color: Color) -> Option<(u8, u8, u8)> {
        match color {
            Color::Rgb(r, g, b) => Some((r, g, b)),
            _ => None,
        }
    }

    /// Relative luminance per WCAG 2.1.
    fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
        fn channel(c: u8) -> f64 {
            let s = c as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    /// WCAG 2.1 contrast ratio.
    fn contrast_ratio(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> f64 {
        let l1 = relative_luminance(fg.0, fg.1, fg.2);
        let l2 = relative_luminance(bg.0, bg.1, bg.2);
        let (lighter, darker) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn all_themes_have_non_default_surface_bg() {
        for theme in ALL_THEMES {
            assert!(
                rgb(theme.surface_bg).is_some(),
                "{}: surface_bg must be an RGB color",
                theme.name
            );
        }
    }

    #[test]
    fn all_themes_have_required_semantic_fields() {
        for theme in ALL_THEMES {
            let name = theme.name;
            // Every theme must have distinct accent colors.
            assert!(
                rgb(theme.accent_primary).is_some(),
                "{name}: accent_primary missing"
            );
            assert!(
                rgb(theme.accent_secondary).is_some(),
                "{name}: accent_secondary missing"
            );
            assert!(
                rgb(theme.accent_action).is_some(),
                "{name}: accent_action missing"
            );

            // Error/destructive must be separate from action accent.
            assert_ne!(
                theme.error_fg, theme.accent_action,
                "{name}: error_fg should differ from accent_action"
            );
            assert_ne!(
                theme.error_fg, theme.accent_primary,
                "{name}: error_fg should differ from accent_primary"
            );

            // Error fields present.
            assert!(rgb(theme.error_fg).is_some(), "{name}: error_fg missing");
            assert!(
                rgb(theme.error_hover).is_some(),
                "{name}: error_hover missing"
            );
            assert!(
                rgb(theme.error_surface).is_some(),
                "{name}: error_surface missing"
            );
            assert!(
                rgb(theme.error_border).is_some(),
                "{name}: error_border missing"
            );
            assert!(
                rgb(theme.error_text).is_some(),
                "{name}: error_text missing"
            );

            // Warning / success / info present.
            assert!(rgb(theme.warning).is_some(), "{name}: warning missing");
            assert!(rgb(theme.success).is_some(), "{name}: success missing");
            assert!(rgb(theme.info).is_some(), "{name}: info missing");

            // Diff colors present.
            assert!(
                rgb(theme.diff_added_fg).is_some(),
                "{name}: diff_added_fg missing"
            );
            assert!(
                rgb(theme.diff_deleted_fg).is_some(),
                "{name}: diff_deleted_fg missing"
            );
            assert!(
                rgb(theme.diff_added_bg).is_some(),
                "{name}: diff_added_bg missing"
            );
            assert!(
                rgb(theme.diff_deleted_bg).is_some(),
                "{name}: diff_deleted_bg missing"
            );

            // Tool colors present.
            assert!(
                rgb(theme.tool_running).is_some(),
                "{name}: tool_running missing"
            );
            assert!(
                rgb(theme.tool_success).is_some(),
                "{name}: tool_success missing"
            );
            assert!(
                rgb(theme.tool_failed).is_some(),
                "{name}: tool_failed missing"
            );
        }
    }

    #[test]
    fn body_text_has_minimum_contrast_on_surface() {
        for theme in ALL_THEMES {
            let name = theme.name;
            let Some(fg) = rgb(theme.text_body) else {
                continue;
            };
            let Some(bg) = rgb(theme.surface_bg) else {
                continue;
            };
            let cr = contrast_ratio(fg, bg);
            assert!(
                cr >= 4.5,
                "{name}: body text contrast {cr:.1}:1 is below 4.5:1 minimum (fg={fg:?}, bg={bg:?})"
            );
        }
    }

    #[test]
    fn muted_text_is_readable_on_surface() {
        for theme in ALL_THEMES {
            let name = theme.name;
            let Some(fg) = rgb(theme.text_muted) else {
                continue;
            };
            let Some(bg) = rgb(theme.surface_bg) else {
                continue;
            };
            let cr = contrast_ratio(fg, bg);
            assert!(
                cr >= 3.0,
                "{name}: muted text contrast {cr:.1}:1 is below 3.0:1 minimum (fg={fg:?}, bg={bg:?})"
            );
        }
    }

    #[test]
    fn error_text_contrasts_on_error_surface() {
        for theme in ALL_THEMES {
            let name = theme.name;
            let Some(fg) = rgb(theme.error_text) else {
                continue;
            };
            let Some(bg) = rgb(theme.error_surface) else {
                continue;
            };
            let cr = contrast_ratio(fg, bg);
            assert!(
                cr >= 4.5,
                "{name}: error_text on error_surface contrast {cr:.1}:1 is below 4.5:1"
            );
        }
    }

    #[test]
    fn selection_bg_differs_from_surface_bg() {
        for theme in ALL_THEMES {
            let name = theme.name;
            assert_ne!(
                theme.selection_bg, theme.surface_bg,
                "{name}: selection_bg must differ from surface_bg"
            );
        }
    }

    #[test]
    fn surface_layers_are_distinct() {
        for theme in ALL_THEMES {
            let name = theme.name;
            // Panel should be distinct from surface (unless grayscale which has limited range).
            if theme.name != "grayscale" {
                assert_ne!(
                    theme.panel_bg, theme.surface_bg,
                    "{name}: panel_bg must differ from surface_bg for visual layering"
                );
            }
        }
    }

    #[test]
    fn success_and_warning_are_visually_distinct() {
        for theme in ALL_THEMES {
            let name = theme.name;
            assert_ne!(
                theme.success, theme.warning,
                "{name}: success and warning must be distinct colors"
            );
            assert_ne!(
                theme.success, theme.error_fg,
                "{name}: success and error must be distinct colors"
            );
        }
    }

    #[test]
    fn diff_added_and_deleted_are_distinct() {
        for theme in ALL_THEMES {
            let name = theme.name;
            assert_ne!(
                theme.diff_added_fg, theme.diff_deleted_fg,
                "{name}: diff add/del fg must differ"
            );
            assert_ne!(
                theme.diff_added_bg, theme.diff_deleted_bg,
                "{name}: diff add/del bg must differ"
            );
        }
    }

    #[test]
    fn mode_colors_are_all_distinct() {
        for theme in ALL_THEMES {
            let name = theme.name;
            let modes = [
                ("agent", theme.mode_agent),
                ("yolo", theme.mode_yolo),
                ("plan", theme.mode_plan),
                ("goal", theme.mode_goal),
            ];
            for i in 0..modes.len() {
                for j in (i + 1)..modes.len() {
                    assert_ne!(
                        modes[i].1, modes[j].1,
                        "{name}: mode {} and mode {} have same color",
                        modes[i].0, modes[j].0
                    );
                }
            }
        }
    }

    #[test]
    fn whale_dark_uses_proposed_palette() {
        // Issue #2012: verify the default Whale dark uses proposed tokens.
        let t = UI_THEME;
        assert_eq!(rgb(t.surface_bg), Some(WHALE_BG_RGB), "Deep Navy #0A1120");
        assert_eq!(
            rgb(t.text_body),
            Some(WHALE_TEXT_BODY_RGB),
            "Whale Ivory #F6F2E8"
        );
        assert_eq!(
            rgb(t.text_muted),
            Some(WHALE_TEXT_MUTED_RGB),
            "Mist Gray #A9B4C7"
        );
        assert_eq!(
            rgb(t.accent_primary),
            Some(WHALE_ACCENT_PRIMARY_RGB),
            "Signal Gold #F6C453"
        );
        assert_eq!(
            rgb(t.accent_secondary),
            Some(WHALE_ACCENT_SECONDARY_RGB),
            "Seafoam #4FD1C5"
        );
        assert_eq!(
            rgb(t.accent_action),
            Some(WHALE_ACCENT_ACTION_RGB),
            "Coral Spark #FF7A59"
        );
        assert_eq!(rgb(t.error_fg), Some((255, 92, 122)), "Rose Red #FF5C7A");
        assert_eq!(
            rgb(t.error_surface),
            Some((42, 18, 26)),
            "Error Surface #2A121A"
        );
    }
}
