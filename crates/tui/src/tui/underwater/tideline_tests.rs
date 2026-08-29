//! Golden-buffer contract for the Tideline startup stage — hero, quick
//! actions, option strip (spec §5a/§5c). Goldens: `startup_{w}x{h}` at the
//! four blocker sizes plus the 40x12 floor. Re-bless with
//! `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    FLUKE_BLOCK, LaunchAction, TidelineStartup, handle_launch_key, render_tideline_startup,
    tideline_startup_hitboxes,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};
use crossterm::event::{KeyCode, KeyModifiers};

fn draw(width: u16, height: u16, startup: &TidelineStartup<'_>) -> String {
    render_golden_text(width, height, |buf| {
        render_tideline_startup(Rect::new(0, 0, width, height), buf, startup)
    })
}

/// The docked composer's display as the real launch screen projects it:
/// blurred, empty, the shared placeholder and refocus hint (Locale::En —
/// the goldens are the English design contract).
fn docked_composer() -> super::LaunchComposerDisplay<'static> {
    let placeholder = crate::localization::tr(
        crate::localization::Locale::En,
        crate::localization::MessageId::ComposerPlaceholder,
    )
    .into_owned();
    let hint_blurred = crate::localization::tr(
        crate::localization::Locale::En,
        crate::localization::MessageId::LaunchComposerFocusHint,
    )
    .into_owned();
    super::LaunchComposerDisplay {
        placeholder: std::borrow::Cow::Owned(placeholder),
        hint_blurred: std::borrow::Cow::Owned(hint_blurred),
        ..super::LaunchComposerDisplay::default()
    }
}

/// The approved startup screen as a deterministic fixture: a returning
/// workspace with a configured provider, the New session quick action
/// focused (the state `tideline_startup_from_app` projects for
/// `launch.selected == 2` — the quick action's launch-table row), and a
/// blurred empty composer docked below the option strip.
fn returning() -> TlineFixture {
    TlineFixture {
        session_count: 4,
        provider_ready: true,
        selected_action: Some(0),
        selected_option: None,
    }
}

struct TlineFixture {
    session_count: usize,
    provider_ready: bool,
    selected_action: Option<usize>,
    selected_option: Option<usize>,
}

impl TlineFixture {
    fn widget<'a>(&self, theme: &'a crate::palette::UiTheme) -> TidelineStartup<'a> {
        let mut startup = TidelineStartup::new(theme, self.session_count, self.provider_ready);
        startup.selected_action = self.selected_action;
        startup.selected_option = self.selected_option;
        startup.composer = docked_composer();
        startup
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
fn startup_matches_golden_at_the_40x12_terminal_floor() {
    // §5b shed order proven at the floor. A 40x12 terminal leaves the stage
    // 10 rows after the topbar and merged footer: the QUICK ACTIONS label
    // row and the wave rules collapse, the hero keeps heading + subtitle
    // (the 12x6 fluke needs its 8-row budget), and the strip sheds to 2
    // columns so tile labels stay whole.
    let fixture = returning();
    let startup = fixture.widget(&UI_THEME);
    let text = draw(40, 10, &startup);
    assert_matches_golden("startup_40x10", &text);
    assert!(text.contains("What are we working on?"), "{text}");
    assert!(
        text.contains("New worktree"),
        "2-column strip keeps tile 1: {text}"
    );
    assert!(
        text.contains("Chat only"),
        "2-column strip keeps tile 2: {text}"
    );
    assert!(!text.contains("Theme"), "tiles 3/4 shed: {text}");
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
fn startup_hero_paints_the_generated_fluke_block_and_sheds_it_at_short_stages() {
    // The generated 12x6 mark (never a hand-drawn crown): centered, one row
    // per FLUKE_BLOCK line, above the heading. It sheds below its 8-row
    // budget instead of being clipped mid-mark.
    let startup = TidelineStartup::new(&UI_THEME, 4, true);
    let wide = draw(80, 24, &startup);
    for row in FLUKE_BLOCK {
        let row_w = unicode_width::UnicodeWidthStr::width(row);
        let expected = format!("{}{row}", " ".repeat((80 - row_w) / 2));
        assert!(
            wide.lines().any(|line| line.starts_with(&expected)),
            "fluke row {row:?} must paint centered at 80x24:\n{wide}"
        );
    }
    // 60x16: stage 14 rows -> hero 5 < 8, mark sheds, heading stays.
    let short = draw(60, 16, &startup);
    assert!(short.contains("What are we working on?"), "{short}");
    assert!(
        !short.contains(FLUKE_BLOCK[0]),
        "the fluke must not clip mid-mark at short stages:\n{short}"
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
fn startup_printed_keys_are_the_keys_the_launch_menu_dispatches() {
    // The stage prints keys on its rows and tiles; those keys must be the
    // ones `handle_launch_key` (main's #5698 input model) actually
    // dispatches — the printed key column cannot drift from the handler.
    // The stage's rows are launch-table rows [2, 4, 1]; its tiles are
    // [3, 4, 5, 6] (worktree, chat, theme, help).
    let mut launch = crate::tui::app::LaunchState {
        visible: false,
        selected: 0,
        worktree_input: None,
        status: None,
        workspace_session_count: 0,
        worktree_available: true,
        row_areas: Vec::new(),
        option_areas: Vec::new(),
        composer_focus: false,
        composer_area: None,
        send_area: None,
    };
    let key = |code: KeyCode, mods: KeyModifiers| crossterm::event::KeyEvent::new(code, mods);
    let none = KeyModifiers::NONE;
    let ctrl = KeyModifiers::CONTROL;
    let locale = crate::localization::Locale::En;

    // Enter on the focused first quick action (New session, table row 2)
    // starts a session.
    launch.selected = 2;
    assert_eq!(
        handle_launch_key(&mut launch, key(KeyCode::Enter, none), locale),
        LaunchAction::NewSession
    );
    // C dispatches chat (the Chat only row and tile share table row 4).
    assert_eq!(
        handle_launch_key(&mut launch, key(KeyCode::Char('c'), none), locale),
        LaunchAction::NewChat
    );
    // Ctrl+R dispatches resume (table row 1 — the third quick action).
    assert_eq!(
        handle_launch_key(&mut launch, key(KeyCode::Char('r'), ctrl), locale),
        LaunchAction::Resume
    );
    // T dispatches the theme picker (the Theme tile's printed key).
    assert_eq!(
        handle_launch_key(&mut launch, key(KeyCode::Char('t'), none), locale),
        LaunchAction::Theme
    );
    // F1 dispatches help (the Help tile's printed key).
    assert_eq!(
        handle_launch_key(&mut launch, key(KeyCode::F(1), none), locale),
        LaunchAction::Help
    );
    // Ctrl+N opens the worktree name prompt (the worktree tile's path).
    launch.selected = 2;
    let action = handle_launch_key(&mut launch, key(KeyCode::Char('n'), ctrl), locale);
    assert!(matches!(action, LaunchAction::None));
    assert!(launch.worktree_input.is_some(), "Ctrl+N opens the prompt");
    launch.worktree_input = None;
    // ↑/↓ navigation spans the launch table: the quick actions (rows 2, 4,
    // 1), the worktree tile (3), and the theme/help tiles (5, 6) are all
    // visible focus targets; only the direct-key chords move beyond.
    launch.selected = 6;
    handle_launch_key(&mut launch, key(KeyCode::Down, none), locale);
    assert_eq!(
        launch.selected, 6,
        "navigation clamps at the last launch-table row (Help)"
    );
    handle_launch_key(&mut launch, key(KeyCode::Up, none), locale);
    assert_eq!(launch.selected, 5, "Up reaches the Theme tile");
    // The stage projection maps every visible row to its table slot and
    // rests nowhere for Connect (row 0 — its P key and the topbar's Model
    // segment are its routes).
    let project = |selected: usize| {
        let quick = super::QUICK_ACTION_ROWS
            .iter()
            .position(|row| *row == selected);
        let tile = if quick.is_some() {
            None
        } else {
            super::OPTION_TILE_ROWS
                .iter()
                .position(|row| *row == selected)
        };
        (quick, tile)
    };
    assert_eq!(project(2), (Some(0), None), "New session");
    assert_eq!(project(4), (Some(1), None), "Chat only — the row wins");
    assert_eq!(project(1), (Some(2), None), "Resume last");
    assert_eq!(project(3), (None, Some(0)), "worktree tile");
    assert_eq!(project(5), (None, Some(2)), "theme tile");
    assert_eq!(project(6), (None, Some(3)), "help tile");
    assert_eq!(project(0), (None, None), "Connect rests nowhere visible");
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
    let mut startup = TidelineStartup::new(&UI_THEME, 4, true);
    startup.selected_option = Some(1);
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
    // The generated fluke projects through the declared quadrant-block
    // fallbacks (`#`, `.`, `\`) — a legible silhouette, not a smear.
    assert!(
        text.lines()
            .any(|line| line.contains("\\###") || line.contains("###.")),
        "ascii fluke block must paint:\n{text}"
    );
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
    let hitboxes = tideline_startup_hitboxes(area);
    assert!(hitboxes.fluke.width > 0, "fluke is a hitbox at 100x30");
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
        let _ = tideline_startup_hitboxes(Rect::new(0, 0, w, h));
    }
}
