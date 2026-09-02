//! Golden-buffer contract for the Tideline merged footer (spec §3 slots
//! 6+8, §5c). Goldens: `footer_{w}x{h}` — the one-row band at the bottom of
//! each blocker-size buffer. Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{
    ChromeInk, TidelineFooter, depth_ink_for, render_tideline_footer, tideline_footer_depth_hitbox,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

const KEYS: &str = "Enter send · Ctrl+K clear · ? help";

fn thinking_footer() -> TelineFixture {
    TelineFixture {
        phase_word: "thinking",
        phase_ink: ChromeInk::Active,
        live_detail: Some("1m 15s"),
        cost_label: "$0.42 · 61K tok",
        context_percent: 61,
        // The old header's posture lockup, carried into the footer per §3.
        mode_chip: Some(("act", ChromeInk::PolicyAct)),
        permission_chip: Some(("ask", ChromeInk::PermissionAsk)),
        notice: None,
    }
}

struct TelineFixture {
    phase_word: &'static str,
    phase_ink: ChromeInk,
    live_detail: Option<&'static str>,
    cost_label: &'static str,
    context_percent: u8,
    mode_chip: Option<(&'static str, ChromeInk)>,
    permission_chip: Option<(&'static str, ChromeInk)>,
    notice: Option<(&'static str, ChromeInk)>,
}

impl TelineFixture {
    fn widget<'a>(&self, theme: &'a crate::palette::UiTheme) -> TidelineFooter<'a> {
        TidelineFooter::new(
            theme,
            self.phase_word,
            self.phase_ink,
            self.cost_label,
            self.context_percent,
            KEYS,
        )
        .live_detail(self.live_detail)
        .mode_chip(self.mode_chip)
        .permission_chip(self.permission_chip)
        .notice(self.notice)
    }
}

fn draw(width: u16, height: u16, footer: &TidelineFooter<'_>) -> String {
    render_golden_text(width, height, |buf| {
        // The shell reserves exactly one bottom row for the footer.
        render_tideline_footer(
            Rect::new(0, height.saturating_sub(1), width, 1),
            buf,
            footer,
        );
    })
}

#[test]
fn footer_matches_goldens_at_blocker_sizes() {
    let fixture = thinking_footer();
    for (w, h) in BLOCKER_SIZES {
        let footer = fixture.widget(&UI_THEME);
        assert_matches_golden(&format!("footer_{w}x{h}"), &draw(w, h, &footer));
    }
}

// The scheduled-work slot moved to the topbar (TUI band contract: work in
// the top strip; the merged footer owns phase/cost/detail). Its topbar
// rendering is pinned by `ui/frame.rs` tests reading the same
// `AutomationPanelState` projection.

#[test]
fn footer_merges_phase_cost_left_and_depth_keys_right() {
    let footer = thinking_footer().widget(&UI_THEME);
    let text = draw(100, 30, &footer);
    let band = text.trim_end().to_string();
    assert!(band.contains("<·> thinking 1m 15s"), "left half: {band}");
    assert!(
        band.contains("$0.42 · 61K tok"),
        "cost joins the left: {band}"
    );
    // The posture chips ride after the cost, whole or not at all.
    assert!(band.contains("$0.42 · 61K tok · act · ask"), "{band}");
    assert!(band.contains("61%"), "depth percent: {band}");
    assert!(band.ends_with(KEYS), "keys legend right: {band}");
}

#[test]
fn footer_keeps_help_discoverable_when_the_full_legend_sheds() {
    let footer = thinking_footer().widget(&UI_THEME);
    let text = draw(80, 24, &footer);
    assert!(text.contains("? help"), "{text}");
    assert!(
        !text.contains("Enter send"),
        "compact footer sheds the chorus: {text}"
    );
}

/// §3: the old header's mode/permission chips move into the footer's left
/// half. A chip that cannot fit whole stands down rather than clipping —
/// the classic header's own rule for posture words.
#[test]
fn footer_posture_chips_fit_whole_or_stand_down() {
    let mut fixture = thinking_footer();
    fixture.permission_chip = Some(("full access", ChromeInk::PermissionFullAccess));
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(
        text.contains("act · full access"),
        "both posture chips render: {text}"
    );

    // Narrow: the chips shed before they would clip.
    let narrow = draw(40, 12, &fixture.widget(&UI_THEME));
    for phrase in ["act · full access", "act ·", "· full access"] {
        assert!(
            !narrow.contains(phrase),
            "narrow row clipped a posture word ({phrase}): {narrow}"
        );
    }
}

/// A live notice owns the trailing right slot over the keys legend — the
/// activity band's toast fact survives the merge (spec §3: nothing the old
/// bands carried is dropped silently).
#[test]
fn footer_notice_owns_the_trailing_slot_over_the_keys() {
    let mut fixture = thinking_footer();
    fixture.notice = Some(("Auto-denied exec_shell", ChromeInk::Attention));
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(text.contains("Auto-denied exec_shell"), "{text}");
    assert!(
        !text.contains(KEYS),
        "the notice outranks the keys legend: {text}"
    );
    assert!(text.contains("61%"), "the depth line stays: {text}");
}

#[test]
fn nonurgent_notice_keeps_compact_help_when_the_floor_fits() {
    let mut fixture = thinking_footer();
    fixture.phase_word = "draft";
    fixture.live_detail = None;
    fixture.notice = Some(("Auto-compaction enabled", ChromeInk::Info));
    let text = draw(60, 16, &fixture.widget(&UI_THEME));
    assert!(text.contains("Auto-compaction enabled"), "{text}");
    assert!(text.contains("? help"), "{text}");
}

#[test]
fn footer_depth_line_is_a_hand_built_ramp_not_a_gauge() {
    let footer = TidelineFooter::new(&UI_THEME, "idle", ChromeInk::Metadata, "$0.00", 61, KEYS);
    let cells = footer.depth_cells();
    assert!(cells.starts_with("▁▂▄▆"), "ramp rises: {cells}");
    assert!(cells.contains('∿'), "open water waves: {cells}");
    assert!(cells.width() <= 16, "depth line stays ≤16 cells");
    // Pure function of the count: same percent, same cells.
    assert_eq!(cells, footer.depth_cells());
}

#[test]
fn footer_warns_at_eighty_percent_cap() {
    let footer = TidelineFooter::new(&UI_THEME, "thinking", ChromeInk::Active, "$0.42", 83, KEYS);
    let text = draw(100, 30, &footer);
    assert!(text.contains("▲"), "cap mark: {text}");
    assert!(
        text.contains("surface soon — /compact"),
        "cap microcopy: {text}"
    );
    assert!(
        !text.contains(KEYS),
        "the warning owns the right side over the keys legend: {text}"
    );
    assert_eq!(depth_ink_for(83), ChromeInk::Attention);
    assert_eq!(depth_ink_for(79), ChromeInk::Info);
}

#[test]
fn footer_idle_has_no_live_detail() {
    let footer = TidelineFooter::new(
        &UI_THEME,
        "idle",
        ChromeInk::Metadata,
        "$0.00 · 0 tok",
        12,
        KEYS,
    );
    let text = draw(100, 30, &footer);
    assert!(text.contains("<·> idle"), "{text}");
    assert!(!text.contains("×"), "no fake live detail: {text}");
}

#[test]
fn footer_ascii_safe_projects_glyphs() {
    let footer = TidelineFooter::new(&UI_THEME, "thinking", ChromeInk::Active, "$0.42", 61, KEYS)
        .ascii_safe(true);
    let text = draw(100, 30, &footer);
    assert!(text.contains("<.>"), "chip projects: {text}");
    assert!(text.contains("__"), "ramp projects to underscores: {text}");
    assert!(text.contains("~~~"), "waves project: {text}");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn footer_depth_hitbox_matches_painted_cells() {
    for (w, h) in BLOCKER_SIZES {
        let footer = thinking_footer().widget(&UI_THEME);
        let band = Rect::new(0, h - 1, w, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        render_tideline_footer(band, &mut buf, &footer);
        let hitbox = tideline_footer_depth_hitbox(band, &footer);
        let cells: String = (hitbox.x..hitbox.x + hitbox.width)
            .map(|x| buf[(x, hitbox.y)].symbol().to_string())
            .collect();
        assert!(
            cells.contains("61%"),
            "depth hitbox covers the painted percent at {w}x{h}: {cells:?}"
        );
        assert!(hitbox.x + hitbox.width <= w);
    }
}

#[test]
fn footer_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (2, 1), (8, 1), (300, 2)] {
        let footer = thinking_footer().widget(&UI_THEME);
        let _ = draw(w, h, &footer);
    }
}
