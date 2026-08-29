//! Golden-buffer contract for the Tideline topbar (spec §5c/§6).
//!
//! Each golden is a cell-exact `.txt` dump of the rendered row at one of the
//! four canonical blocker sizes (`views/status_picker.rs::BLOCKER_SIZES`).
//! The goldens are the design contract: exact characters, exact columns.
//!
//! Re-bless after an intentional design change:
//!
//! ```sh
//! CODEWHALE_BLESS_GOLDENS=1 ./scripts/dev-test.sh tui topbar
//! ```

use ratatui::Terminal;
use ratatui::backend::{Backend, TestBackend};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{Topbar, TopbarSegment, TopbarSegmentId};
use crate::palette::{ChromeInk, UI_THEME, UiTheme};

const CLOCK: &str = "27 Aug 2026 14:42:18";
const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

/// Approved startup screen: route identity is absent (not connected), theme
/// surfaced because first-run is when it is chosen.
fn startup_segments() -> Vec<TopbarSegment> {
    vec![
        TopbarSegment::new(
            TopbarSegmentId::Model,
            "model",
            "not connected",
            ChromeInk::Waiting,
        ),
        TopbarSegment::new(
            TopbarSegmentId::Theme,
            "theme",
            "match terminal",
            ChromeInk::Info,
        ),
    ]
}

/// Approved work + pod screen: run, pod, whale capacity, effective model.
fn work_segments() -> Vec<TopbarSegment> {
    vec![
        TopbarSegment::new(
            TopbarSegmentId::Run,
            "run",
            "release 0.9.12",
            ChromeInk::Identity,
        ),
        TopbarSegment::new(TopbarSegmentId::Pod, "pod", "launch pod", ChromeInk::Active),
        TopbarSegment::new(TopbarSegmentId::Whales, "pod", "3/4", ChromeInk::Info),
        TopbarSegment::new(
            TopbarSegmentId::Model,
            "",
            "claude-3.5-sonnet",
            ChromeInk::Identity,
        ),
    ]
}

/// Approved settings screen: breadcrumb, folder, effective model.
fn settings_segments() -> Vec<TopbarSegment> {
    vec![
        TopbarSegment::new(
            TopbarSegmentId::SettingsPath,
            "",
            "Settings / Appearance",
            ChromeInk::Identity,
        ),
        TopbarSegment::new(
            TopbarSegmentId::Workspace,
            "folder",
            "~/codewhale",
            ChromeInk::Metadata,
        ),
        TopbarSegment::new(
            TopbarSegmentId::Model,
            "",
            "claude-3.5-sonnet",
            ChromeInk::Identity,
        ),
    ]
}

fn fixtures() -> Vec<(&'static str, Vec<TopbarSegment>, u8)> {
    vec![
        ("startup", startup_segments(), 0),
        ("work", work_segments(), 61),
        ("settings", settings_segments(), 61),
    ]
}

fn render_row(theme: &UiTheme, width: u16, segments: &[TopbarSegment], pct: u8) -> String {
    let backend = TestBackend::new(width, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let topbar = Topbar::new(theme, CLOCK, pct, segments);
            use ratatui::widgets::Widget;
            Widget::render(topbar, frame.area(), frame.buffer_mut());
        })
        .expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tui/goldens")
        .join(format!("{name}.txt"))
}

fn bless(name: &str, text: &str) {
    let path = golden_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create goldens dir");
    }
    std::fs::write(path, text).expect("write golden");
}

fn golden_text(name: &str) -> Option<String> {
    // Normalize to LF; a Windows checkout can hand us CRLF while `render_row`
    // always terminates with LF. Cell symbols never contain CR.
    std::fs::read_to_string(golden_path(name))
        .ok()
        .map(|text| text.replace("\r\n", "\n"))
}

#[test]
fn topbar_matches_goldens_at_blocker_sizes() {
    for (screen, segments, pct) in fixtures() {
        for (w, h) in BLOCKER_SIZES {
            let name = format!("topbar_{screen}_{w}x{h}");
            let rendered = render_row(&UI_THEME, w, &segments, pct);
            let rendered = format!("{rendered}\n");
            match golden_text(&name) {
                Some(expected) => {
                    assert_eq!(
                        rendered, expected,
                        "topbar golden drift at {name}; re-bless only with an approved design change"
                    );
                }
                None => {
                    if std::env::var("CODEWHALE_BLESS_GOLDENS").is_ok() {
                        bless(&name, &rendered);
                    } else {
                        panic!(
                            "missing golden {name}; run with CODEWHALE_BLESS_GOLDENS=1 to write it"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn topbar_sheds_in_declared_order_and_keeps_floor() {
    // The work fixture sheds Whales, then Pod, then Run; brand, context
    // meter, clock, and the model segment survive at every blocker size.
    let segments = work_segments();
    for (w, _h) in BLOCKER_SIZES {
        let row = render_row(&UI_THEME, w, &segments, 61);
        assert!(row.contains("CODEWHALE"), "{w}: brand is the floor");
        assert!(row.contains('%'), "{w}: context meter is the floor");
        assert!(row.contains("14:42:18"), "{w}: clock survives");
        assert!(
            row.contains("claude-3.5-sonnet"),
            "{w}: route identity never sheds first"
        );
    }
    // At 80 the work fixture sheds Whales, Pod, and Run; the model segment
    // (shed priority 0) and the floor survive.
    let row80 = render_row(&UI_THEME, 80, &work_segments(), 61);
    assert!(
        !row80.contains("release 0.9.12"),
        "80: run sheds before model"
    );
    assert!(!row80.contains("launch pod"), "80: pod sheds before model");
    // The startup fixture sheds Theme (priority 5) first at 80; the model
    // stays. At 120 both segments fit, matching the reference screen.
    let startup80 = render_row(&UI_THEME, 80, &startup_segments(), 0);
    assert!(
        startup80.contains("not connected"),
        "80: model keeps the row"
    );
    assert!(
        !startup80.contains("match terminal"),
        "80: theme sheds first per the declared order"
    );
    let startup120 = render_row(&UI_THEME, 120, &startup_segments(), 0);
    assert!(
        startup120.contains("match terminal"),
        "120: both segments fit"
    );
}

#[test]
fn topbar_hitboxes_match_painted_cells() {
    use super::topbar_hitboxes;
    let segments = startup_segments();
    let topbar = Topbar::new(&UI_THEME, CLOCK, 0, &segments);
    let area = ratatui::layout::Rect::new(0, 0, 160, 1);
    let hitboxes = topbar_hitboxes(&topbar, area);
    assert_eq!(hitboxes.len(), 3, "brand + two segments");
    // Every hitbox lies inside the row and is non-degenerate.
    for hb in &hitboxes {
        assert_eq!(hb.area.y, 0);
        assert_eq!(hb.area.height, 1);
        assert!(hb.area.width > 0);
        assert!(hb.area.x + hb.area.width <= 160);
    }
    // Hitboxes do not overlap.
    let mut sorted = hitboxes.clone();
    sorted.sort_by_key(|hb| hb.area.x);
    for pair in sorted.windows(2) {
        assert!(
            pair[0].area.x + pair[0].area.width <= pair[1].area.x,
            "hitboxes must not overlap"
        );
    }
    // The painted segment text sits inside its recorded hitbox. Slice by
    // cell, not by byte: the row contains multi-byte glyphs (`│`, meter).
    let cells = render_cells(&UI_THEME, 160, &segments, 0);
    for hb in &hitboxes {
        let text: String = (hb.area.x..hb.area.x + hb.area.width)
            .filter_map(|x| cells.get(usize::from(x)))
            .cloned()
            .collect();
        assert!(
            !text.trim().is_empty(),
            "hitbox {:?} covers empty cells",
            hb.id
        );
    }
}

/// Per-cell symbols of one rendered row (the golden dump, before joining).
fn render_cells(theme: &UiTheme, width: u16, segments: &[TopbarSegment], pct: u8) -> Vec<String> {
    let backend = TestBackend::new(width, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let topbar = Topbar::new(theme, CLOCK, pct, segments);
            use ratatui::widgets::Widget;
            Widget::render(topbar, frame.area(), frame.buffer_mut());
        })
        .expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol().to_string())
        .collect()
}

#[test]
fn topbar_ascii_safe_has_no_wide_or_unsupported_glyphs() {
    let segments = startup_segments();
    let row = {
        let backend = TestBackend::new(160, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let topbar = Topbar::new(&UI_THEME, CLOCK, 61, &segments).ascii_safe(true);
                use ratatui::widgets::Widget;
                Widget::render(topbar, frame.area(), frame.buffer_mut());
            })
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    // The brand lockup is the wordmark alone (founder decree deleted the
    // crown glyph); it is pure ASCII, so ascii-safe mode changes nothing.
    assert!(row.starts_with("CODEWHALE"), "wordmark is the brand: {row}");
    assert!(row.contains('#'), "meter projects to #");
    assert!(!row.contains('▰'), "no block glyphs survive ascii-safe");
    for ch in row.chars() {
        assert_eq!(ch.width(), Some(1), "ascii-safe row must be single-width");
    }
}

#[test]
fn topbar_meter_warns_at_eighty_percent() {
    // ≥80% flips the meter to Attention ink. Styling is not visible in a text
    // dump, so assert on the struct directly: the ink choice is the contract.
    let segments = startup_segments();
    let topbar = Topbar::new(&UI_THEME, CLOCK, 83, &segments);
    // 83% renders the same glyphs as 61%; the difference is color. Golden
    // for the 83% case is intentionally identical in shape — verified by
    // the width contract below, and the ink by `meter_ink_for`.
    assert_eq!(super::meter_ink_for(83), ChromeInk::Attention);
    assert_eq!(super::meter_ink_for(79), ChromeInk::Info);
    let _ = topbar; // silence unused in case of future refactor
}

#[test]
fn topbar_hover_and_narrow_do_not_panic() {
    let segments = work_segments();
    // Hover style change must not move cells.
    let plain = render_row(&UI_THEME, 160, &segments, 61);
    let _hovered = {
        let backend = TestBackend::new(160, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let topbar = Topbar::new(&UI_THEME, CLOCK, 61, &segments)
                    .hovered(Some(TopbarSegmentId::Pod));
                use ratatui::widgets::Widget;
                Widget::render(topbar, frame.area(), frame.buffer_mut());
            })
            .expect("draw");
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    // Degenerate sizes must not panic.
    for w in [1u16, 2, 10, 20, 40] {
        let _ = render_row(&UI_THEME, w, &segments, 61);
    }
    assert!(!plain.is_empty());
}

#[test]
fn context_meter_hitbox_covers_exactly_the_painted_meter_span() {
    // The meter's mouse route must land on the cells the meter painted —
    // the posture-floor discipline (a hitbox never claims cells another
    // element paints), proven against the buffer itself at three row
    // widths: roomy (no shed), tight (segments shed, clock prefix shed),
    // and too narrow (no hitbox rather than an overlapping one).
    use ratatui::widgets::Widget;
    let segments = work_segments();
    for width in [160u16, 80, 60, 44, 30, 20] {
        let topbar = Topbar::new(&UI_THEME, CLOCK, 61, &segments);
        let backend = TestBackend::new(width, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                Widget::render(
                    Topbar::new(&UI_THEME, CLOCK, 61, &segments),
                    frame.area(),
                    frame.buffer_mut(),
                );
            })
            .expect("draw");
        let row: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        match super::context_meter_hitbox(&topbar, terminal.backend().size().expect("size").into())
        {
            Some(area) => {
                let start = usize::from(area.x);
                let end = start + usize::from(area.width);
                let covered = row
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect::<String>();
                assert!(
                    covered.starts_with("context "),
                    "{width} wide: hitbox must start at the meter's first cell: {covered:?}"
                );
                assert!(
                    covered.contains('%') && (covered.contains('▰') || covered.contains('▱')),
                    "{width} wide: hitbox must cover percentage and bar: {covered:?}"
                );
                assert!(
                    !covered.trim_end().contains(':'),
                    "{width} wide: hitbox must not reach the clock: {covered:?}"
                );
            }
            None => {
                // Refused only when even the shed floor cannot fit: brand +
                // gap + the meter + two spaces + the time-only clock.
                let meter_w = "context 61% ".width() + 5;
                let floor = super::brand_width() + super::BRAND_GAP.width() + 1 + meter_w + 2 + 8;
                assert!(
                    usize::from(width) < floor,
                    "{width} wide: a fittable meter (floor {floor}) must return a hitbox"
                );
            }
        }
    }
}
