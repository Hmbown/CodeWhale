//! Golden-buffer contract for the Tideline startup stage — the launch
//! header (mark, wordmark + version, route, workspace, state line) over the
//! docked composer. Goldens: `startup_{w}x{h}` at the four blocker sizes
//! plus the 40x12 floor, `startup_ink_{w}x{h}` for the colour plane, and
//! `startup_surfacing_80x24` for the motion's midpoint. Re-bless with
//! `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    LaunchRecentEntry, MarkTier, McpFacts, TidelineStartup, render_tideline_startup,
    tideline_startup_hitboxes,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{
    BLOCKER_SIZES, assert_matches_golden, render_golden_ink, render_golden_text, render_ink_entries,
};

fn draw(width: u16, height: u16, startup: &TidelineStartup<'_>) -> String {
    render_golden_text(width, height, |buf| {
        let _ = render_tideline_startup(Rect::new(0, 0, width, height), buf, startup);
    })
}

/// The docked composer's display as the real launch screen projects it:
/// focused, empty, the shared placeholder and hint (Locale::En — the
/// goldens are the English design contract).
fn docked_composer() -> super::LaunchComposerDisplay<'static> {
    let placeholder = crate::localization::tr(
        crate::localization::Locale::En,
        crate::localization::MessageId::ComposerPlaceholder,
    )
    .into_owned();
    let hint_focused = crate::localization::tr(
        crate::localization::Locale::En,
        crate::localization::MessageId::LaunchComposerHint,
    )
    .into_owned();
    super::LaunchComposerDisplay {
        focused: true,
        placeholder: std::borrow::Cow::Owned(placeholder),
        hint: std::borrow::Cow::Owned(hint_focused),
        ..super::LaunchComposerDisplay::default()
    }
}

/// The connected launch screen as a deterministic fixture: a route, a
/// forge slug with its branch, two MCP servers up and one waiting on a
/// sign-in, the braille mark settled, the composer focused and empty. The
/// version is pinned so the golden does not churn with every release.
fn connected(theme: &crate::palette::UiTheme) -> TidelineStartup<'_> {
    let mut startup = TidelineStartup::new(
        theme,
        Some("OpenRouter · deepseek-v4".to_string()),
        "Hmbown/CodeWhale · main".to_string(),
    )
    .mcp(Some(McpFacts {
        connected: 2,
        needs_sign_in: 1,
        enabled: 3,
    }))
    .recent(
        vec![
            LaunchRecentEntry {
                id: "sess-aaa".to_string(),
                title: "Fix login flow".to_string(),
                detail: "2h ago · 4 msgs".to_string(),
            },
            LaunchRecentEntry {
                id: "sess-bbb".to_string(),
                title: "Plan export".to_string(),
                detail: "3d ago · 12 msgs".to_string(),
            },
        ],
        false,
    )
    .composer(docked_composer());
    startup.version = "0.9.12";
    startup
}

/// First run: no model, no forge remote — the workspace path stands in.
fn first_run(theme: &crate::palette::UiTheme) -> TidelineStartup<'_> {
    let mut startup = TidelineStartup::new(theme, None, "/Users/hunter/src/codewhale".to_string())
        .composer(docked_composer())
        .composer_rule(Some("no model connected · ask".to_string()));
    startup.version = "0.9.12";
    startup
}

#[test]
fn startup_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        assert_matches_golden(
            &format!("startup_{w}x{h}"),
            &draw(w, h, &connected(&UI_THEME)),
        );
    }
}

#[test]
fn startup_first_run_matches_its_golden() {
    // The founder's own first paint: no model connected, so the card's
    // announcement says so in the gate colour and names the command that
    // fixes it.
    let text = draw(80, 24, &first_run(&UI_THEME));
    assert_matches_golden("startup_first_run_80x24", &text);
    assert!(
        text.contains('⑂'),
        "the top line paints the branch glyph: {text}"
    );
    assert!(
        text.contains("⚠ no model connected · run /provider"),
        "{text}"
    );
    assert!(!text.contains("MCP"), "no MCP news on first run: {text}");
}

#[test]
fn startup_matches_golden_at_the_40x12_terminal_floor() {
    // A 40x12 terminal leaves the stage 10 rows after the topbar and merged
    // footer: the tiny mark, the title, the new-session entry, and the dock
    // all still fit.
    let text = draw(40, 10, &connected(&UI_THEME));
    assert_matches_golden("startup_40x10", &text);
    assert!(text.contains("codewhale"), "{text}");
    assert!(text.contains("New session"), "{text}");
    assert!(text.contains("❯"), "the floor keeps the composer: {text}");
}

#[test]
fn startup_surfacing_midpoint_matches_its_golden() {
    // Half way through the surfacing motion: the mark's lower dot rows are
    // up, its upper rows still under the field, the wordmark half faded in.
    // Reviewable as a golden because progress is injected, not clocked.
    let startup = connected(&UI_THEME).surface_progress(0.5);
    let text = draw(80, 24, &startup);
    assert_matches_golden("startup_surfacing_80x24", &text);
    let settled = draw(80, 24, &connected(&UI_THEME));
    assert_ne!(
        text, settled,
        "the midpoint frame differs from the still frame"
    );
    // The header copy is already in place; only the mark is mid-surface.
    assert!(text.contains("codewhale v0.9.12"), "{text}");
}

#[test]
fn sixel_tier_reserves_a_blank_block_and_reports_it() {
    // The sixel tier paints no ink of its own: a blank 6x3 block the event
    // loop draws the raster over, reported back so the reconciler can
    // position it. Braille and kitty tiers report nothing.
    let area = Rect::new(0, 0, 80, 24);
    let mut sixel = Buffer::empty(area);
    let startup = connected(&UI_THEME).mark(MarkTier::Sixel);
    let reserved = render_tideline_startup(area, &mut sixel, &startup);
    assert_eq!((reserved.width, reserved.height), (6, 3));
    for y in reserved.y..reserved.y + reserved.height {
        for x in reserved.x..reserved.x + reserved.width {
            assert_eq!(
                sixel[(x, y)].symbol(),
                " ",
                "reserve cell ({x},{y}) is blank"
            );
        }
    }
    let mut braille = Buffer::empty(area);
    let settled = render_tideline_startup(area, &mut braille, &connected(&UI_THEME));
    assert_eq!(settled.width, 0, "braille tier reserves no block");
    // The braille still frame carries the founder whale's dots, not blanks.
    let dots = braille_content(&braille);
    assert!(
        dots.chars()
            .any(|glyph| ('\u{2800}'..='\u{28ff}').contains(&glyph)),
        "braille tier still paints dots"
    );
}

/// Collect the card's mark cells as text for tier assertions.
fn braille_content(buf: &Buffer) -> String {
    (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| buf[(x, y)].symbol().to_string()))
        .collect()
}

#[test]
fn the_card_states_the_workspace_recent_work_and_mcp_news() {
    let text = draw(100, 30, &connected(&UI_THEME));
    for fact in [
        "codewhale v0.9.12",
        // The top line owns the workspace truth now.
        "Hmbown/CodeWhale · main",
        // The card's announcement: only when true.
        "● 2 MCP servers connected · 1 needs sign-in · run /mcp",
        // The prominent new-session entry over the recent-work list.
        "New session",
        "Recent",
        "Fix login flow",
        "2h ago",
        "Plan export",
        "3d ago",
    ] {
        assert!(text.contains(fact), "missing {fact:?} in:\n{text}");
    }
    // The new-session entry leads the recent work.
    assert!(
        text.find("New session").unwrap() < text.find("Fix login flow").unwrap(),
        "new session leads the list:\n{text}"
    );
    // The old menu is gone: no rows, no chords.
    for gone in [
        "New worktree",
        "Resume session",
        "Changelog",
        "Quit",
        "ctrl+n",
        "ctrl+r",
        "ctrl+l",
        "ctrl+q",
    ] {
        assert!(!text.contains(gone), "{gone:?} is back:\n{text}");
    }
    // Row 0 is the thin top line; the wordmark lives in the card.
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.contains('⑂'),
        "top line opens with the branch glyph: {first:?}"
    );
    assert!(
        !first.contains("codewhale"),
        "the wordmark left row 0: {first:?}"
    );
    // Nothing from the old stage survives.
    for gone in [
        "QUICK ACTIONS",
        "What are we working on?",
        "⋯ ∼∼∼ ⋯",
        "Tab to type",
        "F1",
    ] {
        assert!(!text.contains(gone), "{gone:?} is back:\n{text}");
    }
}

#[test]
fn startup_state_line_is_silent_when_there_is_nothing_true_to_say() {
    let quiet = connected(&UI_THEME).mcp(None);
    let text = draw(100, 30, &quiet);
    assert!(!text.contains('●') && !text.contains('⚠'), "{text}");
    let idle = connected(&UI_THEME).mcp(Some(McpFacts::default()));
    assert!(!draw(100, 30, &idle).contains("MCP"));
    let one = connected(&UI_THEME).mcp(Some(McpFacts {
        connected: 1,
        needs_sign_in: 0,
        enabled: 1,
    }));
    let text = draw(100, 30, &one);
    assert!(text.contains("● 1 MCP server connected"), "{text}");
    assert!(!text.contains("sign-in"), "{text}");
}

#[test]
fn startup_ascii_safe_drops_the_mark_and_every_wide_glyph() {
    let startup = connected(&UI_THEME).ascii_safe(true);
    assert_eq!(startup.mark, MarkTier::None);
    let text = draw(100, 30, &startup);
    let first = text.lines().next().unwrap_or_default();
    assert!(
        first.trim_start().starts_with('y'),
        "the branch glyph falls back to ASCII on row 0: {first:?}"
    );
    assert!(
        text.contains("codewhale"),
        "the card keeps the wordmark: {text}"
    );
    assert!(
        text.lines().any(|line| {
            let trimmed = line.trim_end();
            trimmed.starts_with('+') && trimmed.ends_with('+') && trimmed.contains('-')
        }),
        "the card border draws in ASCII: {text}"
    );
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(
                ch.width(),
                Some(1),
                "ascii-safe must be single-width: {ch:?}"
            );
            assert!(
                !('\u{2800}'..='\u{28FF}').contains(&ch),
                "no braille in the ASCII lane"
            );
        }
    }
}

#[test]
fn startup_image_tier_places_the_kitty_mark_through_placeholder_cells() {
    let startup = connected(&UI_THEME).mark(MarkTier::Image);
    let area = Rect::new(0, 0, 80, 22);
    let mut buf = Buffer::empty(area);
    render_tideline_startup(area, &mut buf, &startup);
    // The mark lives in the card's left column now, not on row 0: find the
    // placeholder block wherever the card centred it.
    let placeholders = buf
        .content()
        .iter()
        .filter(|cell| cell.symbol().starts_with('\u{10EEEE}'))
        .count();
    assert!(
        placeholders >= 6,
        "kitty placeholders painted: {placeholders}"
    );
    let origin = buf
        .content()
        .iter()
        .find(|cell| cell.symbol().starts_with('\u{10EEEE}'))
        .expect("a placeholder cell");
    assert_eq!(
        origin.fg,
        ratatui::style::Color::Indexed(crate::tui::mark::KITTY_MARK_IMAGE_ID)
    );
}

#[test]
fn startup_hitboxes_match_painted_cells() {
    let startup = connected(&UI_THEME);
    let (w, h) = (100, 30);
    let area = Rect::new(0, 0, w, h);
    let hitboxes = tideline_startup_hitboxes(area);
    let mut buf = Buffer::empty(area);
    render_tideline_startup(area, &mut buf, &startup);
    let input = hitboxes.input.expect("input hitbox");
    let painted: String = (input.x..input.x + input.width)
        .map(|x| buf[(x, input.y)].symbol().to_string())
        .collect();
    assert!(
        !painted.trim().is_empty(),
        "input hitbox covers empty cells"
    );
    let send = hitboxes.send.expect("send hitbox");
    assert!(send.x + send.width <= w && send.y + send.height <= h);
}

#[test]
fn startup_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (1, 1), (4, 3), (7, 5), (9, 5), (200, 50)] {
        let _ = draw(w, h, &connected(&UI_THEME));
        let _ = draw(w, h, &first_run(&UI_THEME).mark(MarkTier::Image));
        let _ = tideline_startup_hitboxes(Rect::new(0, 0, w, h));
    }
}

#[test]
fn startup_ink_plane_matches_goldens_at_blocker_sizes() {
    // The symbol goldens cannot see colour: a screen can go from an accent
    // mark over the field to uniform grey without moving a glyph. This
    // golden is the colour half of that contract.
    for (w, h) in BLOCKER_SIZES {
        let startup = connected(&UI_THEME);
        let rendered = render_golden_ink(w, h, |buf| {
            render_tideline_startup(Rect::new(0, 0, w, h), buf, &startup);
        });
        assert_matches_golden(&format!("startup_ink_{w}x{h}"), &rendered);
    }
}

#[test]
fn startup_paints_the_mark_in_the_accent_and_the_gate_line_in_the_gate_colour() {
    // Anti-regression guard. The mark has been added to this screen and
    // then deleted again twice; a symbol golden cannot defend itself
    // against its own bless, so state the requirement positively.
    let hex = |color: ratatui::style::Color| match color {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        other => panic!("theme slot is not RGB: {other:?}"),
    };
    let accent = hex(UI_THEME.accent_action);
    let (_plane, entries) = render_ink_entries(120, 32, |buf| {
        render_tideline_startup(Rect::new(0, 0, 120, 32), buf, &connected(&UI_THEME));
    });
    let mark_cells: usize = entries
        .iter()
        .filter(|entry| {
            entry.description.starts_with(&accent) && !entry.description.contains("BOLD")
        })
        .map(|entry| entry.glyph_cells)
        .sum();
    assert!(
        mark_cells >= 15,
        "only {mark_cells} accent cells: the mark is gone or degenerate. Inks: {:#?}",
        entries
            .iter()
            .filter(|e| e.glyph_cells > 0)
            .collect::<Vec<_>>()
    );
    // First run: the warning line and `not connected` carry the gate ink,
    // which is distinct from the metadata inks around them.
    let gate = hex(
        crate::palette::chrome_style(&UI_THEME, crate::palette::ChromeInk::Attention)
            .fg
            .expect("attention ink has a foreground"),
    );
    let (_plane, entries) = render_ink_entries(120, 32, |buf| {
        render_tideline_startup(Rect::new(0, 0, 120, 32), buf, &first_run(&UI_THEME));
    });
    let gate_cells: usize = entries
        .iter()
        .filter(|entry| entry.description.starts_with(&gate))
        .map(|entry| entry.glyph_cells)
        .sum();
    assert!(
        gate_cells >= "no model connected".len(),
        "the gate line is not in the gate colour. Inks: {:#?}",
        entries
            .iter()
            .filter(|e| e.glyph_cells > 0)
            .collect::<Vec<_>>()
    );
}
