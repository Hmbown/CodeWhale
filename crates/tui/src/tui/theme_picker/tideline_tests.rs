//! Unit tests for the Tideline theme list (the golden settings-stage suite
//! lives in `views::tideline_tests` — components 9/10/11 share the reserved
//! `settings_*` golden names).

use super::{TidelineThemeList, render_tideline_theme_list, tideline_theme_rows};
use crate::palette::SELECTABLE_THEMES;
use crate::tui::golden_harness::render_golden_text;

#[test]
fn theme_rows_are_the_fourteen_selectable_themes() {
    assert_eq!(tideline_theme_rows().len(), 15);
    assert_eq!(tideline_theme_rows().as_slice(), SELECTABLE_THEMES);
}

#[test]
fn theme_list_selected_row_is_boxed_with_check() {
    let list = TidelineThemeList::new(&crate::palette::UI_THEME, 6);
    let text = render_golden_text(30, 20, |buf| {
        render_tideline_theme_list(ratatui::layout::Rect::new(0, 0, 30, 20), buf, &list);
    });
    let selected_name = tideline_theme_rows()[6].display_name();
    assert!(
        text.contains(&format!("[ ✓ {selected_name} ]")),
        "boxed selection: {text}"
    );
    // Exactly one boxed row.
    assert_eq!(text.matches("[ ✓").count(), 1, "{text}");
}

#[test]
fn theme_list_motion_toggles_reflect_settings() {
    let on = TidelineThemeList::new(&crate::palette::UI_THEME, 0).motion(true, true);
    let text = render_golden_text(30, 20, |buf| {
        render_tideline_theme_list(ratatui::layout::Rect::new(0, 0, 30, 20), buf, &on);
    });
    assert!(text.contains("◉ low motion"), "on mark: {text}");
    assert!(text.contains("◉ ambient life"), "{text}");

    let off = TidelineThemeList::new(&crate::palette::UI_THEME, 0).motion(false, false);
    let text = render_golden_text(30, 20, |buf| {
        render_tideline_theme_list(ratatui::layout::Rect::new(0, 0, 30, 20), buf, &off);
    });
    assert!(text.contains("○ low motion"), "off mark: {text}");
    assert!(text.contains("○ ambient life"), "{text}");
}
