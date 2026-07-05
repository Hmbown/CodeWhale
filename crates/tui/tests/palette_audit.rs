//! Palette audit tests to prevent color drift.
//!
//! These tests ensure that deprecated colors are not used directly in
//! user-visible code. Backward-compatible DeepSeek aliases should point
//! at the current CodeWhale semantic tokens instead of stale brand RGBs.
//!
//! They also audit **every selectable theme preset** for contrast (WCAG
//! relative-luminance ratios over the critical fg/bg pairs) and lint the
//! source tree so raw `Color::Rgb(...)` literals can't leak past the
//! palette layer again.

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
    assert_min_contrast(
        "SELECTION_TEXT on SELECTION_BG",
        palette::SELECTION_TEXT,
        palette::SELECTION_BG,
        min_readable,
    );
    assert_min_contrast(
        "TEXT_PRIMARY on SURFACE_ELEVATED",
        palette::TEXT_PRIMARY,
        palette::SURFACE_ELEVATED,
        min_readable,
    );
    assert_min_contrast(
        "LIGHT_TEXT_BODY on LIGHT_SURFACE",
        palette::LIGHT_TEXT_BODY,
        palette::LIGHT_SURFACE,
        min_readable,
    );
    assert_min_contrast(
        "LIGHT_TEXT_MUTED on LIGHT_SURFACE",
        palette::LIGHT_TEXT_MUTED,
        palette::LIGHT_SURFACE,
        min_readable,
    );
    assert_min_contrast(
        "LIGHT_TEXT_BODY on LIGHT_SELECTION_BG",
        palette::LIGHT_TEXT_BODY,
        palette::LIGHT_SELECTION_BG,
        min_readable,
    );
}

// === Per-preset contrast audit ===
//
// Every concrete preset (10 of the 12 picker entries; `System` is an alias
// for Whale/WhaleLight resolved from the environment and `Terminal` paints
// `Color::Reset` so contrast is owned by the host terminal) must clear the
// following floors over its critical fg/bg pairs:
//
// - body:   4.5:1  — running text (WCAG AA normal text)
// - accent: 3.0:1  — status/accent/badge colors (WCAG AA UI components)
// - dim:    2.0:1  — deliberately de-emphasized text (hints, done-markers);
//                    upstream community palettes (Tokyo Night "comment",
//                    Dracula "comment", …) sit in the 2.2–2.9 band by design
// - border: 1.2:1  — decorative separators; must merely be visible

/// The concrete, RGB-valued presets under audit.
fn audited_themes() -> Vec<palette::UiTheme> {
    palette::SELECTABLE_THEMES
        .iter()
        .filter(|id| !matches!(id, palette::ThemeId::System | palette::ThemeId::Terminal))
        .map(|id| id.ui_theme())
        .collect()
}

#[test]
fn every_preset_passes_contrast_floors() {
    const BODY: f64 = 4.5;
    const ACCENT: f64 = 3.0;
    const DIM: f64 = 2.0;
    const BORDER: f64 = 1.2;

    for theme in audited_themes() {
        let name = theme.name;
        let check = |label: &str, fg: Color, bg: Color, floor: f64| {
            assert_min_contrast(&format!("[{name}] {label}"), fg, bg, floor);
        };

        // Body-text pairs.
        for (surface_label, bg) in [
            ("surface_bg", theme.surface_bg),
            ("panel_bg", theme.panel_bg),
            ("elevated_bg", theme.elevated_bg),
            ("composer_bg", theme.composer_bg),
            ("selection_bg", theme.selection_bg),
        ] {
            check(
                &format!("text_body on {surface_label}"),
                theme.text_body,
                bg,
                BODY,
            );
        }
        for (surface_label, bg) in [
            ("surface_bg", theme.surface_bg),
            ("panel_bg", theme.panel_bg),
        ] {
            check(
                &format!("text_soft on {surface_label}"),
                theme.text_soft,
                bg,
                BODY,
            );
            check(
                &format!("text_muted on {surface_label}"),
                theme.text_muted,
                bg,
                BODY,
            );
        }
        check(
            "error_text on error_surface",
            theme.error_text,
            theme.error_surface,
            BODY,
        );

        // Accent / status pairs on the base surface.
        for (label, fg) in [
            ("accent_primary", theme.accent_primary),
            ("accent_secondary", theme.accent_secondary),
            ("accent_action", theme.accent_action),
            ("warning", theme.warning),
            ("success", theme.success),
            ("info", theme.info),
            ("error_fg", theme.error_fg),
            ("mode_agent", theme.mode_agent),
            ("mode_yolo", theme.mode_yolo),
            ("mode_plan", theme.mode_plan),
            ("mode_goal", theme.mode_goal),
            ("tool_running", theme.tool_running),
            ("tool_failed", theme.tool_failed),
        ] {
            check(
                &format!("{label} on surface_bg"),
                fg,
                theme.surface_bg,
                ACCENT,
            );
        }
        // Footer statusline colors render on footer_bg.
        check(
            "status_working on footer_bg",
            theme.status_working,
            theme.footer_bg,
            ACCENT,
        );
        check(
            "status_warning on footer_bg",
            theme.status_warning,
            theme.footer_bg,
            ACCENT,
        );
        // Diff text on its tinted background and on the plain surface.
        check(
            "diff_added_fg on diff_added_bg",
            theme.diff_added_fg,
            theme.diff_added_bg,
            ACCENT,
        );
        check(
            "diff_deleted_fg on diff_deleted_bg",
            theme.diff_deleted_fg,
            theme.diff_deleted_bg,
            ACCENT,
        );
        check(
            "diff_added_fg on surface_bg",
            theme.diff_added_fg,
            theme.surface_bg,
            ACCENT,
        );
        check(
            "diff_deleted_fg on surface_bg",
            theme.diff_deleted_fg,
            theme.surface_bg,
            ACCENT,
        );

        // Deliberately-dim pairs.
        for (label, fg) in [
            ("text_hint", theme.text_hint),
            ("text_dim", theme.text_dim),
            ("status_ready", theme.status_ready),
            ("tool_success", theme.tool_success),
        ] {
            check(&format!("{label} on surface_bg"), fg, theme.surface_bg, DIM);
        }

        // Borders only need to be visible.
        check(
            "border on surface_bg",
            theme.border,
            theme.surface_bg,
            BORDER,
        );
        check("border on panel_bg", theme.border, theme.panel_bg, BORDER);
    }
}

#[test]
fn every_preset_sets_distinct_deliberate_slots() {
    // Guard against "accidental inherit" mistakes: slots that must never
    // collapse into each other, in any concrete preset. (Some same-value
    // slots ARE deliberate — e.g. grayscale's warning == info — so this
    // list only names pairs whose collision would break the UI.)
    let mut names = std::collections::BTreeSet::new();
    for theme in audited_themes() {
        let name = theme.name;
        assert!(names.insert(name), "duplicate theme name: {name}");
        assert_ne!(
            theme.text_body, theme.surface_bg,
            "[{name}] body == surface"
        );
        assert_ne!(theme.text_body, theme.text_muted, "[{name}] body == muted");
        assert_ne!(
            theme.selection_bg, theme.surface_bg,
            "[{name}] selection invisible"
        );
        assert_ne!(
            theme.status_working, theme.status_warning,
            "[{name}] working == warning"
        );
        assert_ne!(
            theme.tool_running, theme.tool_failed,
            "[{name}] running == failed"
        );
        assert_ne!(
            theme.diff_added_fg, theme.diff_deleted_fg,
            "[{name}] diff +/- collide"
        );
        assert_ne!(
            theme.mode_yolo, theme.mode_goal,
            "[{name}] yolo == goal badge"
        );
        assert_ne!(theme.error_fg, theme.success, "[{name}] error == success");
    }
    // 12 picker entries − System − Terminal = 10 concrete presets.
    assert_eq!(names.len(), palette::SELECTABLE_THEMES.len() - 2);
}

#[test]
fn every_preset_has_display_name_and_tagline() {
    for id in palette::SELECTABLE_THEMES {
        assert!(
            !id.display_name().trim().is_empty(),
            "{id:?} missing display name"
        );
        let tagline = id.tagline();
        assert!(!tagline.trim().is_empty(), "{id:?} missing tagline");
        // Taglines must fit beside the swatches at the picker's minimum
        // width (78 cols − chrome − 22-col name − 10-col swatch).
        assert!(
            tagline.chars().count() <= 34,
            "{id:?} tagline too long for picker row: {tagline:?}"
        );
    }
}

// === NO_COLOR / Ansi16 downsampling sanity ===

#[test]
fn ansi16_downsampling_keeps_status_and_selection_distinguishable() {
    use palette::{ColorDepth, adapt_bg, adapt_color};

    let success = adapt_color(palette::STATUS_SUCCESS, ColorDepth::Ansi16);
    let warning = adapt_color(palette::STATUS_WARNING, ColorDepth::Ansi16);
    let error = adapt_color(palette::STATUS_ERROR, ColorDepth::Ansi16);
    let selection_text = adapt_color(palette::SELECTION_TEXT, ColorDepth::Ansi16);

    // All named (no truecolor escapes may survive under NO_COLOR's floor)…
    for c in [success, warning, error, selection_text] {
        assert!(
            !matches!(c, Color::Rgb(..) | Color::Indexed(_)),
            "Ansi16 must produce named colors, got {c:?}"
        );
    }
    // …and pairwise distinct so status semantics survive downsampling.
    let set = [success, warning, error, selection_text];
    for (i, a) in set.iter().enumerate() {
        for b in set.iter().skip(i + 1) {
            assert_ne!(
                a, b,
                "Ansi16 collapsed two status/selection colors to {a:?}"
            );
        }
    }
    // Background tints are dropped, so selection relies on fg + modifiers;
    // a quiet Reset background beats a coarsely wrong named one.
    assert_eq!(
        adapt_bg(palette::SELECTION_BG, ColorDepth::Ansi16),
        Color::Reset
    );
}

// === Raw-RGB leak lint ===

/// Files allowed to construct `Color::Rgb(...)` directly:
/// - `src/palette.rs` — the palette itself.
/// - `src/deepseek_theme.rs` — theme token tables (currently has none, but
///   it is the second sanctioned color-defining module).
/// Everything else must reach for palette constants / `UiTheme` slots so the
/// light-mode and community-theme remaps can restyle it. Test code (trailing
/// `#[cfg(test)] mod` blocks and `tests.rs` files) is exempt: fixtures often
/// need sentinel RGB values.
#[test]
fn no_raw_rgb_literals_outside_palette_modules() {
    let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowlist: [&str; 2] = ["palette.rs", "deepseek_theme.rs"];

    let mut rust_files = Vec::new();
    collect_rust_files(&src_root, &mut rust_files);
    assert!(
        rust_files.iter().any(|p| p.ends_with("palette.rs")),
        "source scan found no files — lint is miswired"
    );

    let mut violations = Vec::new();
    for path in rust_files {
        let rel = path
            .strip_prefix(&src_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if allowlist.contains(&rel.as_str()) {
            continue;
        }
        // Files that are wholly test modules (`foo/tests.rs`).
        if rel.ends_with("/tests.rs") || rel == "tests.rs" {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("read source file");
        for (line_no, line) in production_lines(&content) {
            if line.contains("Color::Rgb(") {
                violations.push(format!("{rel}:{line_no}: {}", line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "raw Color::Rgb(...) literals outside src/palette.rs / src/deepseek_theme.rs — \
         route them through palette constants or UiTheme slots so theme remaps apply:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Yield `(1-based line number, line)` for production lines only: scanning
/// stops at the first `#[cfg(test)]` attribute that introduces a `mod`
/// (repo convention keeps test modules at the tail of a file).
fn production_lines(content: &str) -> Vec<(usize, &str)> {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed == "#[cfg(test)]" {
            // Look past further attributes/blank lines for a `mod` item.
            let mut j = i + 1;
            while j < lines.len() {
                let next = lines[j].trim();
                if next.is_empty() || next.starts_with("#[") {
                    j += 1;
                    continue;
                }
                break;
            }
            if j < lines.len()
                && (lines[j].trim().starts_with("mod ")
                    || lines[j].trim().starts_with("pub mod ")
                    || lines[j].trim().starts_with("pub(crate) mod "))
            {
                break; // trailing test module — ignore the rest of the file
            }
        }
        result.push((i + 1, lines[i]));
        i += 1;
    }
    result
}
