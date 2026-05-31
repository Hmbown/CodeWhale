//! Palette audit tests to prevent color drift.
//!
//! These tests ensure that deprecated colors (like DEEPSEEK_AQUA) are not used
//! directly in user-visible code. Backward-compatible DeepSeek aliases should
//! point at the current CodeWhale semantic tokens instead of stale brand RGBs.

use std::fs;
use std::path::Path;

use ratatui::style::Color;

#[path = "../src/palette.rs"]
#[allow(dead_code)]
mod palette;

const DEPRECATED_DIRECT_COLORS: &[&str] = &["DEEPSEEK_AQUA"];
const ALLOWED_PATTERNS: &[&str] = &["pub const DEEPSEEK_AQUA", "DEEPSEEK_AQUA_RGB"];

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

fn audit_file(path: &Path, violations: &mut Vec<String>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    for (line_num, line) in content.lines().enumerate() {
        for deprecated in DEPRECATED_DIRECT_COLORS {
            let pattern = format!("palette::{deprecated}");
            if line.contains(&pattern) {
                let is_allowed = ALLOWED_PATTERNS.iter().any(|p| line.contains(p));
                if !is_allowed {
                    violations.push(format!(
                        "{}:{}: direct use of {} (use semantic alias instead)",
                        path.display(),
                        line_num + 1,
                        deprecated
                    ));
                }
            }
        }
    }
}

fn audit_directory(dir: &Path, violations: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            audit_directory(&path, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if path.file_name().is_some_and(|n| n == "palette.rs") {
                continue;
            }
            audit_file(&path, violations);
        }
    }
}

#[test]
fn audit_no_direct_aqua_usage() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = Path::new(manifest_dir).join("src");
    let mut violations = Vec::new();

    audit_directory(&src_dir, &mut violations);

    if !violations.is_empty() {
        let report = violations.join("\n");
        panic!(
            "Palette audit failed! Found {} direct uses of deprecated colors:\n{}",
            violations.len(),
            report
        );
    }
}

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
        palette::DEEPSEEK_AQUA,
        "STATUS_SUCCESS should not regress to deprecated aqua"
    );
}

#[test]
fn verify_brand_aliases_follow_whale_tokens() {
    assert_eq!(palette::WHALE_ACCENT_PRIMARY_RGB, (246, 196, 83));
    assert_eq!(palette::WHALE_INFO_RGB, (106, 174, 242));
    assert_eq!(palette::WHALE_ERROR_RGB, (255, 92, 122));

    assert_eq!(
        palette::DEEPSEEK_BLUE_RGB,
        palette::WHALE_ACCENT_PRIMARY_RGB
    );
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
