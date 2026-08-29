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
    }
}

struct TelineFixture {
    phase_word: &'static str,
    phase_ink: ChromeInk,
    live_detail: Option<&'static str>,
    cost_label: &'static str,
    context_percent: u8,
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
    assert!(band.contains("61%"), "depth percent: {band}");
    assert!(band.ends_with(KEYS), "keys legend right: {band}");
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
