//! Golden-buffer contract for the Tideline composer chrome (spec §2/§5c).
//! Goldens: `composer_{w}x{h}` — the docked 4-row composer at the bottom of
//! each blocker-size buffer. Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    TIDELINE_COMPOSER_HEIGHT, TidelineComposer, render_tideline_composer,
    tideline_composer_hitboxes,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

/// Dock the composer at the bottom of a blocker-size buffer, exactly where
/// the work-screen shell places it (spec §5b: `Constraint::Length(4)`).
fn draw_docked(width: u16, height: u16, composer: &TidelineComposer<'_>) -> String {
    render_golden_text(width, height, |buf| {
        let area = Rect::new(
            0,
            height.saturating_sub(TIDELINE_COMPOSER_HEIGHT),
            width,
            TIDELINE_COMPOSER_HEIGHT,
        );
        render_tideline_composer(area, buf, composer);
    })
}

#[test]
fn composer_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        let composer = TidelineComposer::new(&UI_THEME, "restyle the footer band").focused(true);
        assert_matches_golden(&format!("composer_{w}x{h}"), &draw_docked(w, h, &composer));
    }
}

#[test]
fn composer_border_is_rounded_with_send_hitbox_and_no_crown() {
    let composer = TidelineComposer::new(&UI_THEME, "draft").focused(true);
    let text = draw_docked(80, 24, &composer);
    let top = text.lines().nth(20).unwrap_or_default();
    assert!(top.starts_with('╭'), "rounded top-left: {top:?}");
    assert!(
        top.ends_with('╮'),
        "plain rounded top-right corner: {top:?}"
    );
    assert!(
        !text.contains("▚△▞"),
        "the hand-drawn crown fluke was deleted by the founder decree: {text}"
    );
    let bottom = text.lines().nth(23).unwrap_or_default();
    assert!(
        bottom.starts_with('╰') && bottom.ends_with('╯'),
        "rounded bottom: {bottom:?}"
    );
    let send_row = text.lines().nth(22).unwrap_or_default();
    assert!(send_row.contains("[↑]"), "send hitbox: {send_row:?}");
}

#[test]
fn composer_send_hitbox_survives_a_long_draft() {
    let draft = "x".repeat(240);
    let text = draw_docked(
        80,
        24,
        &TidelineComposer::new(&UI_THEME, &draft).focused(true),
    );
    let send_row = text.lines().nth(22).unwrap_or_default();
    assert!(
        send_row.contains("[↑]"),
        "caller-owned text must not overwrite the submit target: {send_row:?}"
    );
}

#[test]
fn composer_focus_states_change_ink_not_cells() {
    let draft = "same draft";
    let rest = draw_docked(80, 24, &TidelineComposer::new(&UI_THEME, draft));
    let focused = draw_docked(
        80,
        24,
        &TidelineComposer::new(&UI_THEME, draft).focused(true),
    );
    assert_eq!(
        rest, focused,
        "hover/focus restyle must not move a single cell"
    );
}

#[test]
fn composer_pending_crumb_sits_above_the_input_line() {
    let composer = TidelineComposer::new(&UI_THEME, "typed draft")
        .focused(true)
        .pending_crumb(Some("and this after"));
    let text = draw_docked(80, 24, &composer);
    let crumb_row = text.lines().nth(21).unwrap_or_default();
    assert!(crumb_row.contains("queued: and this after"), "{text}");
    let input_row = text.lines().nth(22).unwrap_or_default();
    assert!(input_row.contains("typed draft"), "{text}");
}

#[test]
fn composer_approval_replaces_the_input_line() {
    let composer = TidelineComposer::new(&UI_THEME, "unrelated draft").focused(true);
    let composer = super::TidelineComposer {
        approval_summary: Some("rm -rf target/"),
        ..composer
    };
    let text = draw_docked(80, 24, &composer);
    let input_row = text.lines().nth(21).unwrap_or_default();
    assert!(input_row.contains("approve: rm -rf target/"), "{text}");
    assert!(!input_row.contains("unrelated draft"), "{text}");
}

#[test]
fn composer_ascii_safe_projects_to_ascii() {
    let composer = TidelineComposer::new(&UI_THEME, "draft")
        .focused(true)
        .ascii_safe(true);
    let text = draw_docked(80, 24, &composer);
    let top = text.lines().nth(20).unwrap_or_default();
    assert!(
        top.starts_with('+') && top.ends_with('+'),
        "ascii border corners: {top:?}"
    );
    assert!(!text.contains("<.>"), "ascii crown is gone too: {text}");
    assert!(text.contains("[^]"), "ascii send: {text}");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn composer_hitboxes_match_painted_cells() {
    for (w, h) in BLOCKER_SIZES {
        let area = Rect::new(0, h - TIDELINE_COMPOSER_HEIGHT, w, TIDELINE_COMPOSER_HEIGHT);
        let composer = TidelineComposer::new(&UI_THEME, "draft").focused(true);
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        render_tideline_composer(area, &mut buf, &composer);
        let hitboxes = tideline_composer_hitboxes(area);
        assert_eq!(
            hitboxes.border, area,
            "focus hitbox must cover the full rounded shell at {w}x{h}"
        );
        let submit: String = (hitboxes.submit.x..hitboxes.submit.x + hitboxes.submit.width)
            .map(|x| buf[(x, hitboxes.submit.y)].symbol().to_string())
            .collect();
        assert_eq!(
            submit, "[↑]",
            "submit rect covers the painted hitbox at {w}x{h}"
        );
        let border: String = (hitboxes.border.x..hitboxes.border.x + hitboxes.border.width)
            .map(|x| buf[(x, hitboxes.border.y)].symbol().to_string())
            .collect();
        assert!(
            border.contains('╭'),
            "border rect covers the top rail at {w}x{h}"
        );
    }
}

#[test]
fn composer_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (2, 2), (5, 3), (6, 3), (10, 4), (300, 6)] {
        let composer = TidelineComposer::new(&UI_THEME, "x");
        let area = Rect::new(0, 0, w, h);
        let mut buf = Buffer::empty(Rect::new(0, 0, w.max(1), h.max(1)));
        render_tideline_composer(area, &mut buf, &composer);
    }
}
