//! Golden-buffer contract for the posture bar — the first row under the
//! composer. Goldens: `footer_{w}x{h}` — the one-row band at the bottom of
//! each blocker-size buffer. Re-bless by DELETING the golden and running
//! with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{ChromeInk, TidelineFooter, render_tideline_footer};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

struct Fixture {
    permission: (&'static str, ChromeInk),
    permission_key: Option<&'static str>,
    mode: Option<(&'static str, ChromeInk)>,
    mode_key: Option<&'static str>,
    counts: Vec<(String, ChromeInk)>,
    hint: Option<(&'static str, ChromeInk)>,
    context_percent: u8,
    right: Option<(&'static str, ChromeInk)>,
}

/// A working turn with two sub-agents, ask posture, work mode.
fn working() -> Fixture {
    Fixture {
        permission: ("ask", ChromeInk::PermissionAsk),
        permission_key: Some("Shift+Tab"),
        mode: Some(("work", ChromeInk::PolicyAct)),
        mode_key: Some("Tab"),
        counts: vec![("2 agents".to_string(), ChromeInk::Active)],
        hint: Some(("Esc to interrupt", ChromeInk::MetadataHint)),
        context_percent: 61,
        right: None,
    }
}

impl Fixture {
    fn widget<'a>(&'a self, theme: &'a crate::palette::UiTheme) -> TidelineFooter<'a> {
        TidelineFooter::new(theme, self.permission)
            .permission_key(self.permission_key)
            .mode_chip(self.mode)
            .mode_key(self.mode_key)
            .counts(&self.counts)
            .hint(self.hint)
            .context_percent(self.context_percent)
            .right(self.right)
    }
}

fn draw(width: u16, height: u16, footer: &TidelineFooter<'_>) -> String {
    render_golden_text(width, height, |buf| {
        // The shell reserves exactly one row for the bar.
        render_tideline_footer(
            Rect::new(0, height.saturating_sub(1), width, 1),
            buf,
            footer,
        );
    })
}

#[test]
fn footer_matches_goldens_at_blocker_sizes() {
    let fixture = working();
    for (w, h) in BLOCKER_SIZES {
        let footer = fixture.widget(&UI_THEME);
        assert_matches_golden(&format!("footer_{w}x{h}"), &draw(w, h, &footer));
    }
}

/// Claude Code's grammar: mark, permission chip with its cycle key, mode
/// with its cycle key, live counts, then the one hint that applies now.
#[test]
fn posture_bar_reads_permission_mode_counts_hint() {
    let text = draw(100, 30, &working().widget(&UI_THEME));
    let band = text.lines().last().unwrap_or_default().trim_end();
    assert_eq!(
        band,
        "▶▶ ask (Shift+Tab) · work (Tab) · 2 agents · Esc to interrupt"
    );
}

/// The bar carries no phase word, no elapsed, no cost and no context
/// reading: the transcript, the roster and the metrics line own those.
#[test]
fn posture_bar_states_no_phase_cost_or_context_reading() {
    for pct in [0u8, 12, 61, 79] {
        let mut fixture = working();
        fixture.context_percent = pct;
        let text = draw(120, 32, &fixture.widget(&UI_THEME));
        assert!(!text.contains(&format!("{pct}%")), "{pct}: {text}");
        assert!(!text.contains("thinking"), "{text}");
        assert!(!text.contains("<·>"), "{text}");
        assert!(!text.contains('$'), "{text}");
        assert!(!text.contains("1m 15s"), "{text}");
    }
}

/// At the cap the bar's hint slot says what to do, outranking the
/// interrupt hint; the reading itself still is not here.
#[test]
fn posture_bar_warns_at_eighty_percent_cap() {
    let mut fixture = working();
    fixture.context_percent = 83;
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(text.contains("▲ surface soon — /compact"), "{text}");
    assert!(!text.contains("Esc to interrupt"), "{text}");
    assert!(!text.contains("83%"), "{text}");
}

/// The right slot is the notice when one is owed, else the remote-control
/// state; it never covers the permission chip.
#[test]
fn posture_bar_pins_notice_or_remote_control_right() {
    let mut fixture = working();
    fixture.right = Some(("/rc connected", ChromeInk::Info));
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(text.trim_end().ends_with("/rc connected"), "{text}");
    assert!(text.contains("▶▶ ask (Shift+Tab)"), "{text}");

    fixture.right = Some(("Auto-denied exec_shell", ChromeInk::Attention));
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(
        text.trim_end().ends_with("Auto-denied exec_shell"),
        "{text}"
    );

    // Narrow: the notice truncates against the permission floor, never
    // over it.
    let narrow = draw(30, 12, &fixture.widget(&UI_THEME));
    assert!(narrow.contains("▶▶ ask"), "{narrow}");
}

/// Shed ladder, most expendable first: hint, counts, mode key, mode,
/// permission key. The permission chip never sheds (#5796).
#[test]
fn posture_bar_sheds_hint_counts_mode_key_mode_then_permission_key() {
    let fixture = working();
    let narrowest_showing = |needle: &str| -> u16 {
        (8..=120u16)
            .filter(|w| draw(*w, 3, &fixture.widget(&UI_THEME)).contains(needle))
            .min()
            .unwrap_or_else(|| panic!("{needle} never painted"))
    };
    let hint = narrowest_showing("Esc to interrupt");
    let counts = narrowest_showing("2 agents");
    let mode_key = narrowest_showing("work (Tab)");
    let mode = narrowest_showing("work");
    let permission_key = narrowest_showing("(Shift+Tab)");
    assert!(
        hint > counts && counts > mode_key && mode_key > mode && mode > permission_key,
        "hint@{hint} counts@{counts} mode_key@{mode_key} mode@{mode} permission_key@{permission_key}"
    );
    for w in 8..=120u16 {
        let text = draw(w, 3, &fixture.widget(&UI_THEME));
        assert!(
            text.contains("ask"),
            "{w}: the permission chip never sheds: {text}"
        );
        // Whole chips or none — a clipped posture word is worse than none.
        assert!(
            !text.contains("(Ta") || text.contains("(Tab)"),
            "{w}: {text}"
        );
    }
}

/// Permission outranks mode when only one posture chip fits: the longest
/// mode word must never displace `full access`.
#[test]
fn posture_bar_permission_outranks_mode_when_only_one_fits() {
    let mut fixture = working();
    fixture.permission = ("full access", ChromeInk::PermissionFullAccess);
    fixture.mode = Some(("operate", ChromeInk::PolicyOperate));
    let mut saw_permission_alone = false;
    for width in 8..=120u16 {
        let text = draw(width, 12, &fixture.widget(&UI_THEME));
        let has_mode = text.contains("operate");
        let has_permission = text.contains("full access");
        assert!(
            !(has_mode && !has_permission),
            "width {width} kept the mode word and shed the permission phrase: {text}"
        );
        if has_permission && !has_mode {
            saw_permission_alone = true;
        }
    }
    assert!(saw_permission_alone, "no width shed the mode word alone");
}

/// The cycle keys print only when the caller says the binding is live —
/// the launch stage's Tab moves focus, so the mode chip there has no key.
#[test]
fn posture_bar_prints_cycle_keys_only_when_live() {
    let mut fixture = working();
    fixture.mode_key = None;
    fixture.permission_key = None;
    let text = draw(100, 30, &fixture.widget(&UI_THEME));
    assert!(text.contains("▶▶ ask · work · 2 agents"), "{text}");
    assert!(!text.contains('('), "{text}");
}

#[test]
fn posture_bar_ascii_safe_projects_glyphs() {
    let mut fixture = working();
    fixture.context_percent = 90;
    let text = draw(100, 30, &fixture.widget(&UI_THEME).ascii_safe(true));
    let band = text.lines().last().unwrap_or_default();
    assert!(band.starts_with(">> ask"), "mark projects: {band}");
    assert!(text.contains("^ surface soon"), "{text}");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn posture_bar_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (2, 1), (8, 1), (300, 2)] {
        let fixture = working();
        let _ = draw(w, h, &fixture.widget(&UI_THEME));
    }
}
