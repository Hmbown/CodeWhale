//! Golden-buffer contract for the Tideline settings stage — settings rail
//! (component 11) + theme list (component 9) + live preview (component 10)
//! per spec §5a/§5b/§5c. Goldens: `settings_{w}x{h}` at the four blocker
//! sizes; the 80-wide golden proves the preview pane sheds and the category
//! strip replaces the rail. Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::tideline_preview::TidelineSettingsPreview;
use super::{
    ConfigCategory, TidelineSettingsRail, TidelineSettingsStage, render_tideline_settings_rail,
    render_tideline_settings_stage, render_tideline_settings_strip, tideline_settings_categories,
    tideline_settings_rail_hitboxes,
};
use crate::localization::Locale;
use crate::palette::{SELECTABLE_THEMES, UI_THEME};
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};
use crate::tui::theme_picker::{
    TidelineThemeList, render_tideline_theme_list, tideline_theme_list_hitboxes,
};

fn draw_stage(width: u16, height: u16) -> String {
    render_golden_text(width, height, |buf| {
        let rail = TidelineSettingsRail {
            theme: &UI_THEME,
            selected: 0,
            ascii_safe: false,
            locale: Locale::En,
        };
        let theme_list = TidelineThemeList::new(&UI_THEME, 4).motion(false, true);
        let preview = TidelineSettingsPreview {
            active_theme: &UI_THEME,
            candidate: &UI_THEME,
            candidate_name: "Blue Stage",
            ascii_safe: false,
        };
        let stage = TidelineSettingsStage {
            rail,
            theme_list,
            preview,
        };
        render_tideline_settings_stage(Rect::new(0, 0, width, height), buf, &stage);
    })
}

/// Goldens are stored without cell padding: every row is right-trimmed and
/// trailing empty rows are dropped, so `git diff --check` stays clean.
fn trim_rows(text: &str) -> String {
    let mut rows: Vec<&str> = text.lines().map(str::trim_end).collect();
    while rows.last().is_some_and(|row| row.is_empty()) {
        rows.pop();
    }
    let mut out = rows.join("\n");
    out.push('\n');
    out
}

#[test]
fn settings_stage_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        assert_matches_golden(&format!("settings_{w}x{h}"), &trim_rows(&draw_stage(w, h)));
    }
}

#[test]
fn settings_rail_lists_seven_categories_and_meta_rows() {
    let rail = TidelineSettingsRail {
        theme: &UI_THEME,
        selected: 0,
        ascii_safe: false,
        locale: Locale::En,
    };
    let text = render_golden_text(20, 30, |buf| {
        render_tideline_settings_rail(Rect::new(0, 0, 20, 30), buf, &rail);
    });
    let categories = tideline_settings_categories(Locale::En);
    assert_eq!(categories.len(), 7);
    // One taxonomy: the stage rail lists exactly ConfigView's categories.
    for (label, category) in categories.iter().zip(ConfigCategory::ALL) {
        assert_eq!(label.as_ref(), category.label(Locale::En).as_ref());
        assert!(text.contains(label.as_ref()), "missing {label}: {text}");
    }
    assert!(text.contains("Models & providers"), "{text}");
    assert!(text.contains("Tools & MCP"), "{text}");
    assert!(text.contains("Advanced"), "{text}");
    assert!(text.contains("? help"), "{text}");
    assert!(text.contains("/ file issue"), "{text}");
    assert!(text.contains("f feedback"), "{text}");
    assert!(text.contains("▸ Appearance"), "selected marker: {text}");
}

#[test]
fn settings_strip_windows_to_the_selected_category_with_painted_hitboxes() {
    for (width, selected) in [(38u16, 6usize), (44, 6), (60, 4), (76, 0), (96, 6)] {
        let rail = TidelineSettingsRail {
            theme: &UI_THEME,
            selected,
            ascii_safe: false,
            locale: Locale::En,
        };
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        let boxes = render_tideline_settings_strip(area, &mut buf, &rail);
        let text: String = (0..width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        let label = ConfigCategory::ALL[selected].label(Locale::En);
        assert!(
            text.contains(label.as_ref()),
            "{width} cols must show the active category {label}: {text}"
        );
        assert!(!boxes.is_empty() && boxes.len() <= 8, "{width}: {boxes:?}");
        for rect in &boxes {
            assert!(rect.right() <= area.right(), "{width}: {rect:?} overflows");
            let cells: String = (rect.x..rect.right())
                .map(|x| buf[(x, rect.y)].symbol().to_string())
                .collect();
            assert!(
                !cells.trim().is_empty(),
                "{width}: strip rect covers empty cells"
            );
        }
    }
}

#[test]
fn theme_list_shows_fourteen_themes_boxed_selection_and_motion_toggles() {
    let list = TidelineThemeList::new(&UI_THEME, 4).motion(false, true);
    let text = render_golden_text(30, 24, |buf| {
        render_tideline_theme_list(Rect::new(0, 0, 30, 24), buf, &list);
    });
    assert_eq!(SELECTABLE_THEMES.len(), 15, "4 mode rows + 11 presets");
    for name in [
        "System",
        "Terminal",
        "Blue Stage",
        "Catppuccin Mocha",
        "Uwu",
    ] {
        assert!(text.contains(name), "missing {name}: {text}");
    }
    // Index 4 is Blue Stage; Underwater + Underwater Retro sit between
    // Terminal and Blue Stage.
    assert!(text.contains("Underwater"), "{text}");
    assert!(text.contains("Underwater Retro"), "{text}");
    assert!(
        text.contains("[ ✓ Blue Stage ]"),
        "selected row boxed with check: {text}"
    );
    assert!(text.contains("MOTION (OPTIONAL)"), "{text}");
    assert!(text.contains("low motion"), "{text}");
    assert!(text.contains("ambient life"), "{text}");
}

#[test]
fn live_preview_is_a_real_projection_of_the_real_renderers() {
    // The preview pane paints the actual stream/ledger/composer/footer
    // glyphs — proof by content, not by mock.
    let preview = TidelineSettingsPreview {
        active_theme: &UI_THEME,
        candidate: &UI_THEME,
        candidate_name: "Blue Stage",
        ascii_safe: false,
    };
    let text = render_golden_text(60, 30, |buf| {
        super::tideline_preview::render_tideline_settings_preview(
            Rect::new(0, 0, 60, 30),
            buf,
            &preview,
        );
    });
    assert!(text.contains("PREVIEW · Blue Stage"), "{text}");
    assert!(text.contains("└── whale-2"), "real pod tree: {text}");
    assert!(text.contains("● working"), "real receipt marks: {text}");
    assert!(text.contains("FLEET LEDGER"), "real ledger: {text}");
    assert!(text.contains("╭"), "real composer chrome: {text}");
    assert!(text.contains("╮"), "real composer corner: {text}");
    assert!(
        !text.contains("▚△▞"),
        "crown fluke deleted by founder decree: {text}"
    );
    assert!(text.contains("[↑]"), "real send hitbox: {text}");
    // The posture bar's own mark and permission chip (the metrics line owns
    // the context reading; the transcript owns the phase word).
    assert!(
        text.contains("▶▶ ask (Shift+Tab)"),
        "real footer band: {text}"
    );
}

#[test]
fn settings_stage_sheds_preview_below_hundred_columns() {
    let wide = draw_stage(120, 32);
    assert!(
        wide.contains("PREVIEW ·"),
        "preview pane present at 120: {wide}"
    );
    let narrow = draw_stage(80, 24);
    assert!(
        !narrow.contains("PREVIEW ·"),
        "preview sheds below 100 cols: {narrow}"
    );
    assert!(
        narrow.contains("Appearance"),
        "category strip stays: {narrow}"
    );
    assert!(
        !narrow.contains("? help"),
        "no vertical rail below 100 cols: {narrow}"
    );
    assert!(narrow.contains("MOTION (OPTIONAL)"), "form stays: {narrow}");
}

#[test]
fn settings_rail_and_theme_list_hitboxes_match_painted_rows() {
    let rail = TidelineSettingsRail {
        theme: &UI_THEME,
        selected: 2,
        ascii_safe: false,
        locale: Locale::En,
    };
    let (w, h) = (120, 32);
    let nav = Rect::new(0, 0, 20, h);
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
    render_tideline_settings_rail(nav, &mut buf, &rail);
    for rect in tideline_settings_rail_hitboxes(nav, &rail) {
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(!cells.trim().is_empty(), "rail rect covers empty cells");
    }

    let list = TidelineThemeList::new(&UI_THEME, 0).motion(false, true);
    let form = Rect::new(20, 0, 30, h);
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
    render_tideline_theme_list(form, &mut buf, &list);
    let boxes = tideline_theme_list_hitboxes(form, &list);
    assert_eq!(boxes.len(), 17, "15 theme rows + 2 motion toggles");
    for rect in &boxes {
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(!cells.trim().is_empty(), "theme rect covers empty cells");
    }
}

#[test]
fn settings_stage_ascii_safe_is_single_width() {
    let text = render_golden_text(120, 32, |buf| {
        let rail = TidelineSettingsRail {
            theme: &UI_THEME,
            selected: 0,
            ascii_safe: true,
            locale: Locale::En,
        };
        let theme_list = TidelineThemeList::new(&UI_THEME, 1)
            .motion(true, false)
            .ascii_safe(true);
        let preview = TidelineSettingsPreview {
            active_theme: &UI_THEME,
            candidate: &UI_THEME,
            candidate_name: "Blue Stage",
            ascii_safe: true,
        };
        let stage = TidelineSettingsStage {
            rail,
            theme_list,
            preview,
        };
        render_tideline_settings_stage(Rect::new(0, 0, 120, 32), buf, &stage);
    });
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn settings_stage_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (10, 2), (29, 4), (300, 60)] {
        let _ = draw_stage(w, h);
    }
}
