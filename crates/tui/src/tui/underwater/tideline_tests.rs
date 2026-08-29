//! Golden-buffer contract for the Tideline startup stage — hero, quick
//! actions, option strip (spec §5a/§5c). Goldens: `startup_{w}x{h}` at the
//! four blocker sizes. Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{TidelineStartup, render_tideline_startup, tideline_startup_hitboxes};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

fn draw(width: u16, height: u16, startup: &TidelineStartup<'_>) -> String {
    render_golden_text(width, height, |buf| {
        render_tideline_startup(Rect::new(0, 0, width, height), buf, startup)
    })
}

/// The approved startup screen as a deterministic fixture: a returning
/// workspace with a configured provider, first action focused.
fn returning() -> TlineFixture {
    TlineFixture {
        session_count: 4,
        provider_ready: true,
        selected_action: 0,
        selected_option: 0,
    }
}

struct TlineFixture {
    session_count: usize,
    provider_ready: bool,
    selected_action: usize,
    selected_option: usize,
}

impl TlineFixture {
    fn widget<'a>(&self, theme: &'a crate::palette::UiTheme) -> TidelineStartup<'a> {
        TidelineStartup::new(theme, self.session_count, self.provider_ready)
            .selected_action(self.selected_action)
            .selected_option(self.selected_option)
    }
}

#[test]
fn startup_matches_goldens_at_blocker_sizes() {
    let fixture = returning();
    for (w, h) in BLOCKER_SIZES {
        let startup = fixture.widget(&UI_THEME);
        assert_matches_golden(&format!("startup_{w}x{h}"), &draw(w, h, &startup));
    }
}

#[test]
fn startup_hero_states_first_run_vs_returning() {
    let first_run = TidelineStartup::new(&UI_THEME, 0, false);
    let text = draw(100, 30, &first_run);
    assert!(text.contains("What are we working on?"), "{text}");
    assert!(
        text.contains("type below, or pick a first move"),
        "first-run subtitle: {text}"
    );
    let returning = TidelineStartup::new(&UI_THEME, 4, true);
    let text = draw(100, 30, &returning);
    assert!(
        text.contains("welcome back · 4 saved sessions"),
        "returning subtitle: {text}"
    );
}

#[test]
fn startup_quick_actions_carry_icon_description_command_and_chevron() {
    let startup = TidelineStartup::new(&UI_THEME, 4, true);
    let text = draw(100, 30, &startup);
    for fact in [
        "QUICK ACTIONS",
        "New session",
        "start a fresh agent run",
        "Enter ›",
        "Chat only",
        "Resume last",
        "Ctrl+R ›",
    ] {
        assert!(text.contains(fact), "missing {fact:?} in:\n{text}");
    }
}

#[test]
fn startup_disabled_rows_render_dimmer_set_not_hidden() {
    // No provider and no saved sessions: chat-only and resume are disabled
    // but still readable — availability is state, not absence.
    let startup = TidelineStartup::new(&UI_THEME, 0, false);
    let text = draw(100, 30, &startup);
    assert!(text.contains("Chat only"), "{text}");
    assert!(text.contains("Resume last"), "{text}");
}

#[test]
fn startup_option_strip_sheds_to_two_columns_when_narrow() {
    let startup = TidelineStartup::new(&UI_THEME, 4, true).selected_option(1);
    let wide = draw(80, 24, &startup);
    for tile in ["New worktree", "Chat only", "Theme", "Help"] {
        assert!(wide.contains(tile), "80 cols shows all four tiles: {wide}");
    }
    let narrow = draw(30, 20, &startup);
    assert!(narrow.contains("New worktree"), "narrow keeps tile 1");
    assert!(narrow.contains("Chat only"), "narrow keeps tile 2");
    assert!(!narrow.contains("Theme\n"), "narrow sheds tiles 3/4");
}

#[test]
fn startup_ascii_safe_has_no_wide_or_unsupported_glyphs() {
    let startup = TidelineStartup::new(&UI_THEME, 4, true).ascii_safe(true);
    let text = draw(100, 30, &startup);
    assert!(text.contains("<.>"), "fluke projects to <.>");
    assert!(text.contains(". ~~~ ."), "wave rule projects to ASCII");
    assert!(text.contains("Enter >"), "chevron projects to >");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(
                ch.width(),
                Some(1),
                "ascii-safe must be single-width: {ch:?}"
            );
        }
    }
}

#[test]
fn startup_hitboxes_match_painted_cells() {
    let startup = returning().widget(&UI_THEME);
    let (w, h) = (100, 30);
    let area = Rect::new(0, 0, w, h);
    let hitboxes = tideline_startup_hitboxes(area, &startup);
    assert!(hitboxes.fluke.width > 0, "fluke is a hitbox (opens menu)");
    assert_eq!(hitboxes.actions.len(), 3, "one rect per quick action row");
    assert_eq!(hitboxes.options.len(), 4, "one rect per option tile");
    let mut buf = Buffer::empty(area);
    render_tideline_startup(area, &mut buf, &startup);
    let painted = |rect: Rect| -> String {
        (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect()
    };
    for rect in hitboxes.actions.iter().copied().chain(hitboxes.options) {
        assert!(
            !painted(rect).trim().is_empty(),
            "hitbox {rect:?} covers empty cells"
        );
        assert!(rect.x + rect.width <= w);
        assert!(rect.y + rect.height <= h);
    }
    let fluke_cells = painted(hitboxes.fluke);
    assert!(
        !fluke_cells.trim().is_empty(),
        "fluke hitbox covers the mark"
    );
}

#[test]
fn startup_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (1, 1), (4, 3), (7, 5), (200, 50)] {
        let startup = TidelineStartup::new(&UI_THEME, 1, true);
        let _ = draw(w, h, &startup);
    }
}
