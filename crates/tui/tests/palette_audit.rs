//! Palette audit tests to prevent color drift.
//!
//! These tests ensure that deprecated colors are not used directly in
//! user-visible code. Backward-compatible DeepSeek aliases should point
//! at the current CodeWhale semantic tokens instead of stale brand RGBs.

use ratatui::style::Color;

#[path = "../src/palette.rs"]
#[allow(dead_code)]
mod palette;

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (169, 169, 169),
        Color::Red => (255, 0, 0),
        Color::LightRed => (255, 102, 102),
        Color::Green => (0, 255, 0),
        Color::LightGreen => (102, 255, 102),
        Color::Yellow => (255, 255, 0),
        Color::LightYellow => (255, 255, 153),
        Color::Blue => (0, 0, 255),
        Color::LightBlue => (102, 153, 255),
        Color::Magenta => (255, 0, 255),
        Color::LightMagenta => (255, 153, 255),
        Color::Cyan => (0, 255, 255),
        Color::LightCyan => (153, 255, 255),
        _ => panic!("unsupported color variant for contrast test: {color:?}"),
    }
}

fn linearize_srgb(component: u8) -> f64 {
    let srgb = f64::from(component) / 255.0;
    if srgb <= 0.04045 {
        srgb / 12.92
    } else {
        ((srgb + 0.055) / 1.055).powf(2.4)
    }
}

fn relative_luminance(color: Color) -> f64 {
    let (r, g, b) = color_to_rgb(color);
    0.2126 * linearize_srgb(r) + 0.7152 * linearize_srgb(g) + 0.0722 * linearize_srgb(b)
}

fn contrast_ratio(foreground: Color, background: Color) -> f64 {
    let fg = relative_luminance(foreground);
    let bg = relative_luminance(background);
    if fg >= bg {
        (fg + 0.05) / (bg + 0.05)
    } else {
        (bg + 0.05) / (fg + 0.05)
    }
}

fn assert_min_contrast(label: &str, foreground: Color, background: Color, min_ratio: f64) {
    let ratio = contrast_ratio(foreground, background);
    assert!(
        ratio >= min_ratio,
        "{label} contrast {ratio:.2} is below minimum {min_ratio:.2}"
    );
}

// NOTE: The deprecated color audit (DEEPSEEK_AQUA) was removed because
// the deprecated constant no longer exists in the palette.

#[test]
fn verify_status_success_uses_success_token() {
    assert_eq!(
        palette::STATUS_SUCCESS,
        Color::Rgb(
            palette::WHALE_SUCCESS_RGB.0,
            palette::WHALE_SUCCESS_RGB.1,
            palette::WHALE_SUCCESS_RGB.2
        ),
        "STATUS_SUCCESS should use the current success token"
    );
    assert_ne!(
        palette::STATUS_SUCCESS,
        palette::WHALE_ACCENT_PRIMARY,
        "STATUS_SUCCESS should not regress to the primary accent"
    );
}

#[test]
#[allow(deprecated)]
fn verify_brand_aliases_follow_whale_tokens() {
    assert_eq!(palette::WHALE_ACCENT_PRIMARY_RGB, (246, 196, 83));
    assert_eq!(palette::WHALE_INFO_RGB, (106, 174, 242));
    assert_eq!(palette::WHALE_ERROR_RGB, (255, 92, 122));
    assert_eq!(
        color_to_rgb(palette::WHALE_ACCENT_PRIMARY),
        palette::WHALE_ACCENT_PRIMARY_RGB
    );

    assert_eq!(
        palette::DEEPSEEK_BLUE_RGB,
        palette::WHALE_ACCENT_PRIMARY_RGB
    );
    assert_eq!(palette::DEEPSEEK_BLUE, palette::WHALE_ACCENT_PRIMARY);
    assert_eq!(palette::DEEPSEEK_SKY_RGB, palette::WHALE_INFO_RGB);
    assert_eq!(palette::DEEPSEEK_RED_RGB, palette::WHALE_ERROR_RGB);
}

#[test]
fn contrast_guardrails_for_key_ui_pairs() {
    let min_readable = 4.5;

    assert_min_contrast(
        "TEXT_BODY on DEEPSEEK_INK",
        palette::TEXT_BODY,
        palette::DEEPSEEK_INK,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_SECONDARY on DEEPSEEK_INK",
        palette::TEXT_SECONDARY,
        palette::DEEPSEEK_INK,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_HINT on DEEPSEEK_INK",
        palette::TEXT_HINT,
        palette::DEEPSEEK_INK,
        min_readable,
    );
    assert_min_contrast(
        "STATUS_WARNING on DEEPSEEK_INK",
        palette::STATUS_WARNING,
        palette::DEEPSEEK_INK,
        min_readable,
    );
    assert_min_contrast(
        "STATUS_ERROR on DEEPSEEK_INK",
        palette::STATUS_ERROR,
        palette::DEEPSEEK_INK,
        min_readable,
    );
}
