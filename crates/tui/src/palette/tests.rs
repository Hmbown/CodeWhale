use super::adapt::{
    ColorDepth, adapt_bg, adapt_bg_for_palette_mode, adapt_bg_for_theme, adapt_color,
    adapt_fg_for_palette_mode, adapt_fg_for_theme, blend, luma, nearest_ansi16, pulse_brightness,
    reasoning_surface_tint, rgb_to_ansi256,
};
use super::detect::{PaletteMode, palette_mode_from_apple_interface_style};
use super::themes::{
    GRAYSCALE_UI_THEME, LIGHT_UI_THEME, SOLARIZED_LIGHT_UI_THEME, TERMINAL_UI_THEME, ThemeId,
    UI_THEME, UiTheme, normalize_hex_rgb_color, normalize_theme_name, parse_hex_rgb_color,
    theme_label_for_mode, ui_theme_from_settings,
};
use super::tokens::{
    ACCENT_REASONING_LIVE, DIFF_ADDED, DIFF_ADDED_BG, GRAYSCALE_BORDER, GRAYSCALE_ELEVATED,
    GRAYSCALE_PANEL, GRAYSCALE_REASONING, GRAYSCALE_SURFACE, GRAYSCALE_TEXT_BODY,
    GRAYSCALE_TEXT_HINT, GRAYSCALE_TEXT_SOFT, LIGHT_BORDER, LIGHT_ELEVATED, LIGHT_PANEL,
    LIGHT_REASONING, LIGHT_SURFACE, LIGHT_TEXT_BODY, LIGHT_TEXT_BODY_RGB, LIGHT_TEXT_HINT,
    SOLARIZED_PANEL, SOLARIZED_SURFACE, SOLARIZED_TEXT_BODY, SOLARIZED_TEXT_HINT,
    SURFACE_REASONING, SURFACE_REASONING_TINT, TEXT_BODY, TEXT_HINT, TEXT_REASONING,
    TEXT_TOOL_OUTPUT, WHALE_BG, WHALE_ERROR, WHALE_INFO, WHALE_PANEL, WHALE_REASONING_TEXT_RGB,
    WHALE_REASONING_TINT_RGB, WHALE_TEXT_BODY_RGB,
};
use ratatui::style::Color;

#[test]
fn palette_mode_parses_colorfgbg_background_slot() {
    assert_eq!(
        PaletteMode::from_colorfgbg("0;15"),
        Some(PaletteMode::Light)
    );
    assert_eq!(PaletteMode::from_colorfgbg("15;0"), Some(PaletteMode::Dark));
    assert_eq!(
        PaletteMode::from_colorfgbg("7;default;15"),
        Some(PaletteMode::Light)
    );
    assert_eq!(PaletteMode::from_colorfgbg("not-a-color"), None);
}

#[test]
fn palette_mode_detect_prefers_colorfgbg_over_macos_fallback() {
    assert_eq!(
        PaletteMode::detect_from_sources(Some("0;15"), Some(PaletteMode::Dark)),
        PaletteMode::Light
    );
    assert_eq!(
        PaletteMode::detect_from_sources(Some("15;0"), Some(PaletteMode::Light)),
        PaletteMode::Dark
    );
}

#[test]
fn palette_mode_detect_uses_macos_fallback_when_colorfgbg_missing_or_invalid() {
    assert_eq!(
        PaletteMode::detect_from_sources(None, Some(PaletteMode::Light)),
        PaletteMode::Light
    );
    assert_eq!(
        PaletteMode::detect_from_sources(Some("not-a-color"), Some(PaletteMode::Light)),
        PaletteMode::Light
    );
    assert_eq!(
        PaletteMode::detect_from_sources(None, None),
        PaletteMode::Dark
    );
}

#[test]
fn apple_interface_style_maps_dark_and_missing_key_to_expected_modes() {
    assert_eq!(
        palette_mode_from_apple_interface_style("Dark\n"),
        PaletteMode::Dark
    );
    assert_eq!(
        palette_mode_from_apple_interface_style("Light\n"),
        PaletteMode::Light
    );
    assert_eq!(
        palette_mode_from_apple_interface_style(""),
        PaletteMode::Light
    );
}

#[test]
fn ui_theme_selects_light_variant() {
    let theme = UiTheme::for_mode(PaletteMode::Light);
    assert_eq!(theme, LIGHT_UI_THEME);
    assert_eq!(theme.surface_bg, LIGHT_SURFACE);
    assert_eq!(theme.text_body, LIGHT_TEXT_BODY);
}

#[test]
fn ui_theme_selects_grayscale_variant() {
    let theme = UiTheme::for_mode(PaletteMode::Grayscale);
    assert_eq!(theme, GRAYSCALE_UI_THEME);
    assert_eq!(theme.surface_bg, GRAYSCALE_SURFACE);
    assert_eq!(theme.panel_bg, GRAYSCALE_PANEL);
    assert_eq!(theme.text_body, GRAYSCALE_TEXT_BODY);
}

#[test]
fn ui_theme_selects_solarized_light_variant() {
    let theme = UiTheme::for_mode(PaletteMode::SolarizedLight);
    assert_eq!(theme, SOLARIZED_LIGHT_UI_THEME);
    assert_eq!(theme.surface_bg, SOLARIZED_SURFACE);
    assert_eq!(theme.panel_bg, SOLARIZED_PANEL);
    assert_eq!(theme.text_body, SOLARIZED_TEXT_BODY);
}

#[test]
fn theme_names_normalize_common_grayscale_aliases() {
    assert_eq!(normalize_theme_name("system"), Some("system"));
    assert_eq!(normalize_theme_name("default"), Some("system"));
    assert_eq!(normalize_theme_name("whale"), Some("dark"));
    assert_eq!(normalize_theme_name("transparent"), Some("terminal"));
    assert_eq!(normalize_theme_name("inherit"), Some("terminal"));
    assert_eq!(normalize_theme_name("black-white"), Some("grayscale"));
    assert_eq!(normalize_theme_name("mono"), Some("grayscale"));
    assert_eq!(normalize_theme_name("solarized"), Some("solarized-light"));
    assert_eq!(theme_label_for_mode(PaletteMode::Grayscale), "grayscale");
}

#[test]
fn terminal_theme_resets_surfaces_and_remaps_direct_palette_constants() {
    assert_eq!(ThemeId::from_name("terminal"), Some(ThemeId::Terminal));
    assert_eq!(TERMINAL_UI_THEME.surface_bg, Color::Reset);
    assert_eq!(TERMINAL_UI_THEME.footer_bg, Color::Reset);
    assert_eq!(TERMINAL_UI_THEME.text_body, Color::Reset);

    assert_eq!(
        adapt_bg_for_theme(WHALE_BG, ThemeId::Terminal, &TERMINAL_UI_THEME),
        Color::Reset
    );
    assert_eq!(
        adapt_bg_for_theme(DIFF_ADDED_BG, ThemeId::Terminal, &TERMINAL_UI_THEME),
        Color::Reset
    );
    assert_eq!(
        adapt_fg_for_theme(TEXT_BODY, ThemeId::Terminal, &TERMINAL_UI_THEME),
        Color::Reset
    );
    assert_eq!(
        adapt_fg_for_theme(DIFF_ADDED, ThemeId::Terminal, &TERMINAL_UI_THEME),
        Color::Green
    );
}

#[test]
fn light_palette_has_quiet_layer_separation() {
    assert_eq!(LIGHT_SURFACE, Color::Rgb(246, 248, 251));
    assert_eq!(LIGHT_PANEL, Color::Rgb(236, 242, 248));
    assert_eq!(LIGHT_ELEVATED, Color::Rgb(219, 229, 240));
    assert_eq!(LIGHT_BORDER, Color::Rgb(139, 161, 184));
    assert_ne!(LIGHT_SURFACE, LIGHT_PANEL);
    assert_ne!(LIGHT_PANEL, LIGHT_ELEVATED);
}

#[test]
fn solarized_light_does_not_mutate_whale_light_text() {
    assert_eq!(
        LIGHT_TEXT_BODY,
        Color::Rgb(
            LIGHT_TEXT_BODY_RGB.0,
            LIGHT_TEXT_BODY_RGB.1,
            LIGHT_TEXT_BODY_RGB.2
        )
    );
    assert_ne!(LIGHT_TEXT_BODY, SOLARIZED_TEXT_BODY);
}

#[test]
fn dark_palette_uses_soft_body_text_and_warm_reasoning() {
    assert_eq!(
        TEXT_BODY,
        Color::Rgb(
            WHALE_TEXT_BODY_RGB.0,
            WHALE_TEXT_BODY_RGB.1,
            WHALE_TEXT_BODY_RGB.2
        )
    );
    assert_eq!(
        TEXT_REASONING,
        Color::Rgb(
            WHALE_REASONING_TEXT_RGB.0,
            WHALE_REASONING_TEXT_RGB.1,
            WHALE_REASONING_TEXT_RGB.2
        )
    );
    assert_eq!(
        ACCENT_REASONING_LIVE,
        Color::Rgb(
            WHALE_REASONING_TEXT_RGB.0,
            WHALE_REASONING_TEXT_RGB.1,
            WHALE_REASONING_TEXT_RGB.2
        )
    );
    assert_ne!(TEXT_REASONING, TEXT_TOOL_OUTPUT);
    assert_ne!(TEXT_BODY, Color::White);
}

#[test]
fn ui_theme_applies_custom_background_to_base_surfaces() {
    let custom = Color::Rgb(26, 27, 38);
    let theme = UiTheme::for_mode(PaletteMode::Dark).with_background_color(custom);

    assert_eq!(theme.surface_bg, custom);
    assert_eq!(theme.header_bg, custom);
    assert_eq!(theme.footer_bg, custom);
    assert_eq!(
        theme.composer_bg, UI_THEME.composer_bg,
        "custom background must not erase panel contrast"
    );
}

#[test]
fn hex_rgb_color_parser_accepts_hashless_and_normalizes() {
    assert_eq!(parse_hex_rgb_color("#1a1B26"), Some(Color::Rgb(26, 27, 38)));
    assert_eq!(parse_hex_rgb_color("1a1b26"), Some(Color::Rgb(26, 27, 38)));
    assert_eq!(
        normalize_hex_rgb_color("#1A1B26").as_deref(),
        Some("#1a1b26")
    );
    assert_eq!(parse_hex_rgb_color("#123"), None);
    assert_eq!(parse_hex_rgb_color("#zzzzzz"), None);
}

#[test]
fn light_palette_maps_dark_surfaces_and_text() {
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_BG, PaletteMode::Light),
        LIGHT_SURFACE
    );
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_PANEL, PaletteMode::Light),
        LIGHT_PANEL
    );
    assert_eq!(
        adapt_fg_for_palette_mode(Color::White, LIGHT_SURFACE, PaletteMode::Light),
        LIGHT_TEXT_BODY
    );
    assert_eq!(
        adapt_fg_for_palette_mode(TEXT_HINT, LIGHT_SURFACE, PaletteMode::Light),
        LIGHT_TEXT_HINT
    );
}

#[test]
fn solarized_light_palette_maps_dark_surfaces_and_text_to_solarized_roles() {
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_BG, PaletteMode::SolarizedLight),
        SOLARIZED_SURFACE
    );
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_PANEL, PaletteMode::SolarizedLight),
        SOLARIZED_PANEL
    );
    assert_eq!(
        adapt_fg_for_palette_mode(Color::White, SOLARIZED_SURFACE, PaletteMode::SolarizedLight),
        SOLARIZED_TEXT_BODY
    );
    assert_eq!(
        adapt_fg_for_palette_mode(TEXT_HINT, SOLARIZED_SURFACE, PaletteMode::SolarizedLight),
        SOLARIZED_TEXT_HINT
    );
}

#[test]
fn grayscale_palette_maps_brand_hues_to_neutral_roles() {
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_BG, PaletteMode::Grayscale),
        GRAYSCALE_SURFACE
    );
    assert_eq!(
        adapt_bg_for_palette_mode(WHALE_PANEL, PaletteMode::Grayscale),
        GRAYSCALE_PANEL
    );
    assert_eq!(
        adapt_bg_for_palette_mode(SURFACE_REASONING, PaletteMode::Grayscale),
        GRAYSCALE_REASONING
    );
    assert_eq!(
        adapt_fg_for_palette_mode(WHALE_INFO, GRAYSCALE_SURFACE, PaletteMode::Grayscale),
        GRAYSCALE_TEXT_SOFT
    );
    assert_eq!(
        adapt_fg_for_palette_mode(WHALE_ERROR, GRAYSCALE_SURFACE, PaletteMode::Grayscale),
        GRAYSCALE_TEXT_BODY
    );
    assert_eq!(
        adapt_fg_for_palette_mode(TEXT_HINT, GRAYSCALE_SURFACE, PaletteMode::Grayscale),
        GRAYSCALE_TEXT_HINT
    );
}

#[test]
fn grayscale_luma_handles_bright_rgb_without_overflow() {
    assert_eq!(luma(255, 255, 255), 255);
    assert_eq!(
        adapt_fg_for_palette_mode(
            Color::Rgb(255, 255, 255),
            GRAYSCALE_SURFACE,
            PaletteMode::Grayscale
        ),
        GRAYSCALE_TEXT_BODY
    );
}

#[test]
fn ui_theme_from_settings_applies_theme_and_background() {
    let theme = ui_theme_from_settings("grayscale", Some("#111111"));
    assert_eq!(theme.mode, PaletteMode::Grayscale);
    assert_eq!(theme.surface_bg, Color::Rgb(17, 17, 17));
    assert_eq!(theme.header_bg, Color::Rgb(17, 17, 17));
    assert_eq!(theme.footer_bg, Color::Rgb(17, 17, 17));
    assert_eq!(theme.panel_bg, GRAYSCALE_PANEL);
    assert_eq!(theme.elevated_bg, GRAYSCALE_ELEVATED);
    assert_eq!(theme.border, GRAYSCALE_BORDER);
}

#[test]
fn adapt_color_passes_through_truecolor() {
    let c = Color::Rgb(53, 120, 229);
    assert_eq!(adapt_color(c, ColorDepth::TrueColor), c);
}

#[test]
fn adapt_color_maps_rgb_to_indexed_on_ansi256() {
    let c = Color::Rgb(53, 120, 229);
    assert!(matches!(
        adapt_color(c, ColorDepth::Ansi256),
        Color::Indexed(_)
    ));
}

#[test]
fn adapt_bg_maps_rgb_to_indexed_on_ansi256() {
    assert!(matches!(
        adapt_bg(SURFACE_REASONING, ColorDepth::Ansi256),
        Color::Indexed(_)
    ));
}

#[test]
fn adapt_color_drops_to_named_on_ansi16() {
    // Sky: blue-dominant and bright → LightBlue, not terminal cyan.
    assert_eq!(
        adapt_color(WHALE_INFO, ColorDepth::Ansi16),
        Color::LightBlue
    );
    // Rose Red is intentionally bright enough to use the terminal's
    // bright red slot.
    assert_eq!(
        adapt_color(WHALE_ERROR, ColorDepth::Ansi16),
        Color::LightRed
    );
}

#[test]
fn adapt_bg_disables_tints_on_ansi16() {
    assert_eq!(
        adapt_bg(SURFACE_REASONING, ColorDepth::Ansi16),
        Color::Reset
    );
    assert_eq!(
        adapt_bg(SURFACE_REASONING, ColorDepth::TrueColor),
        SURFACE_REASONING
    );
}

#[test]
fn reasoning_tint_is_none_on_ansi16() {
    assert!(reasoning_surface_tint(ColorDepth::Ansi16).is_none());
    assert!(reasoning_surface_tint(ColorDepth::TrueColor).is_some());
    assert!(matches!(
        reasoning_surface_tint(ColorDepth::Ansi256),
        Some(Color::Indexed(_))
    ));
}

#[test]
fn light_palette_maps_reasoning_tint_to_light_surface() {
    assert_eq!(
        SURFACE_REASONING_TINT,
        Color::Rgb(
            WHALE_REASONING_TINT_RGB.0,
            WHALE_REASONING_TINT_RGB.1,
            WHALE_REASONING_TINT_RGB.2
        )
    );
    assert_eq!(
        adapt_bg_for_palette_mode(SURFACE_REASONING_TINT, PaletteMode::Light),
        LIGHT_REASONING
    );
    assert_eq!(
        adapt_bg_for_palette_mode(
            reasoning_surface_tint(ColorDepth::TrueColor).expect("truecolor tint"),
            PaletteMode::Light,
        ),
        LIGHT_REASONING
    );
}

#[test]
fn blend_at_zero_returns_bg_at_one_returns_fg() {
    let fg = Color::Rgb(200, 100, 50);
    let bg = Color::Rgb(0, 0, 0);
    assert_eq!(blend(fg, bg, 0.0), bg);
    assert_eq!(blend(fg, bg, 1.0), fg);
}

#[test]
fn blend_at_half_is_midpoint() {
    let mid = blend(Color::Rgb(200, 100, 0), Color::Rgb(0, 0, 0), 0.5);
    assert_eq!(mid, Color::Rgb(100, 50, 0));
}

#[test]
fn pulse_brightness_swings_within_envelope() {
    // The pulse rides between 30%..100% — never below 30% of the source.
    let src = ACCENT_REASONING_LIVE;
    let mut min_r = u8::MAX;
    let mut max_r = 0u8;
    for ms in (0u64..2000).step_by(50) {
        if let Color::Rgb(r, _, _) = pulse_brightness(src, ms) {
            min_r = min_r.min(r);
            max_r = max_r.max(r);
        }
    }
    let Color::Rgb(src_r, _, _) = src else {
        panic!("expected RGB");
    };
    // Trough should land near 30% of source; crest near source itself.
    let lower = (f32::from(src_r) * 0.30).round() as u8;
    assert!(min_r <= lower + 2, "trough too high: {min_r}");
    assert!(max_r + 2 >= src_r, "crest too low: {max_r}");
}

#[test]
fn pulse_passes_named_colors_unchanged() {
    // Named palette entries don't blend meaningfully — leave them alone.
    assert_eq!(pulse_brightness(Color::Reset, 0), Color::Reset);
    assert_eq!(pulse_brightness(Color::Cyan, 1234), Color::Cyan);
}

#[test]
fn nearest_ansi16_routes_known_brand_colors() {
    // v0.8.45: accent primary is Signal Gold (#F6C453), secondary is Seafoam.
    assert_eq!(nearest_ansi16(246, 196, 83), Color::LightYellow); // Signal Gold
    assert_eq!(nearest_ansi16(79, 209, 197), Color::LightCyan); // Seafoam
    assert_eq!(nearest_ansi16(42, 74, 127), Color::Blue); // Border
    assert_eq!(nearest_ansi16(54, 187, 212), Color::LightCyan); // Aqua
    assert_eq!(nearest_ansi16(255, 92, 122), Color::LightRed); // Rose Red
    assert_eq!(nearest_ansi16(13, 21, 37), Color::Black); // Deep Navy
}

#[test]
fn rgb_to_ansi256_uses_stable_extended_palette() {
    assert!(rgb_to_ansi256(53, 120, 229) >= 16);
    assert!(rgb_to_ansi256(11, 21, 38) >= 16);
}

#[test]
fn color_depth_detect_is_safe_without_env() {
    // Don't try to pin the result — env may be anything in CI. Just
    // exercise the path so a panic would surface.
    let _ = ColorDepth::detect();
    let _ = adapt_color(WHALE_BG, ColorDepth::detect());
}
