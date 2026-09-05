//! Golden-buffer contract for the metrics line — the row under the posture
//! bar.
//!
//! Goldens live in `crates/tui/src/tui/goldens/infoline_{screen}_{w}x{h}.txt`
//! for the two screens (startup, work) at the blocker sizes. Re-bless by
//! deleting the golden and running with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use unicode_width::UnicodeWidthStr;

use super::{InfoLine, InfoSegment, InfoSegmentId, context_meter_hitbox, infoline_hitboxes};
use crate::palette::{ChromeInk, UI_THEME, UiTheme};

/// The hint the live shell advertises, from the one binding module that owns
/// it — a fixture string here would let chrome and routing drift apart.
fn help_hint() -> String {
    crate::tui::shell_key_routing::info_help_hint(crate::localization::Locale::En)
}

const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

fn context(pct: u8) -> InfoSegment {
    InfoSegment::new(
        InfoSegmentId::Context,
        "ctx",
        format!("{pct}%"),
        if pct >= 80 {
            ChromeInk::Failure
        } else {
            ChromeInk::Info
        },
    )
}

/// Approved startup screen: no route yet, no metrics yet.
fn startup_segments() -> Vec<InfoSegment> {
    vec![
        InfoSegment::new(
            InfoSegmentId::Model,
            "",
            "model not connected",
            ChromeInk::Waiting,
        ),
        context(0),
    ]
}

/// Approved work screen: model, context, cost, then the session metrics.
fn work_segments() -> Vec<InfoSegment> {
    vec![
        InfoSegment::new(InfoSegmentId::Model, "", "deepseek-v4", ChromeInk::Identity),
        context(61),
        InfoSegment::new(InfoSegmentId::Cost, "", "$0.42", ChromeInk::MetadataValue),
        InfoSegment::new(
            InfoSegmentId::Ttft,
            "ttft",
            "400ms",
            ChromeInk::MetadataValue,
        ),
        InfoSegment::new(
            InfoSegmentId::Rate,
            "",
            "38 tok/s",
            ChromeInk::MetadataValue,
        ),
        InfoSegment::new(
            InfoSegmentId::OutputTokens,
            "↓",
            "1.2K",
            ChromeInk::MetadataValue,
        ),
    ]
}

fn fixtures() -> Vec<(&'static str, Vec<InfoSegment>)> {
    vec![("startup", startup_segments()), ("work", work_segments())]
}

fn render_buffer(theme: &UiTheme, width: u16, segments: &[InfoSegment]) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let hint = help_hint();
    terminal
        .draw(|frame| {
            let info = InfoLine::new(theme, &hint, segments);
            use ratatui::widgets::Widget;
            Widget::render(info, frame.area(), frame.buffer_mut());
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn render_row(theme: &UiTheme, width: u16, segments: &[InfoSegment]) -> String {
    render_cells(theme, width, segments).concat()
}

/// Per-cell symbols of one rendered row (the golden dump, before joining).
fn render_cells(theme: &UiTheme, width: u16, segments: &[InfoSegment]) -> Vec<String> {
    render_buffer(theme, width, segments)
        .content()
        .iter()
        .map(|cell| cell.symbol().to_string())
        .collect()
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
fn infoline_matches_goldens_at_blocker_sizes() {
    for (screen, segments) in fixtures() {
        for (w, h) in BLOCKER_SIZES {
            let name = format!("infoline_{screen}_{w}x{h}");
            let rendered = render_row(&UI_THEME, w, &segments);
            let rendered = format!("{rendered}\n");
            match golden_text(&name) {
                Some(expected) => {
                    assert_eq!(
                        rendered, expected,
                        "info-line golden drift at {name}; re-bless only with an approved design change"
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

/// The row states no time of day, carries no wordmark, and no longer names
/// the repository or branch: the launch header and the git bottom view own
/// those (2026-09-02).
#[test]
fn infoline_is_model_context_and_metrics_only() {
    for (_, segments) in fixtures() {
        for (w, _h) in BLOCKER_SIZES {
            let row = render_row(&UI_THEME, w, &segments);
            assert!(
                !row.contains(':'),
                "{w}: the metrics line carries no clock: {row:?}"
            );
            assert!(
                !row.contains("CODEWHALE") && !row.contains("codewhale"),
                "{w}: no wordmark or repository on this row: {row:?}"
            );
            assert!(!row.contains('⑂'), "{w}: no branch on this row: {row:?}");
        }
    }
    let work = render_row(&UI_THEME, 160, &work_segments());
    assert!(
        work.starts_with("deepseek-v4 · ctx 61% · $0.42 · ttft 400ms · 38 tok/s · ↓ 1.2K  "),
        "{work:?}"
    );
    assert!(work.trim_end().ends_with("Ctrl+/ help"), "{work:?}");
}

/// Declared shed order: `tok/s`, `ttft`, `↓ tokens`, the help hint, then
/// the cost. The model and `ctx NN%` are the floor at every width.
#[test]
fn infoline_sheds_rate_then_ttft_then_tokens_then_help_then_cost() {
    let segments = work_segments();
    // The narrowest row that still shows a thing. A thing that sheds earlier
    // needs a wider row to survive, so these strictly decrease down the
    // declared order.
    let narrowest_showing = |needle: &str| -> u16 {
        (24..=180u16)
            .filter(|w| render_row(&UI_THEME, *w, &segments).contains(needle))
            .min()
            .unwrap_or_else(|| panic!("{needle} never painted at any width"))
    };
    let rate = narrowest_showing("tok/s");
    let ttft = narrowest_showing("ttft");
    let tokens = narrowest_showing("↓ 1.2K");
    let help = narrowest_showing("help");
    let cost = narrowest_showing("$0.42");
    assert!(
        rate > ttft && ttft > tokens && tokens > help && help > cost,
        "shed order broke: rate@{rate} ttft@{ttft} tokens@{tokens} help@{help} cost@{cost}"
    );
    for w in 24..=180u16 {
        let row = render_row(&UI_THEME, w, &segments);
        assert!(
            row.contains("deepseek-v4") && row.contains("ctx 61%"),
            "{w}: the model and the context reading never shed: {row:?}"
        );
    }
}

/// At the 80% cap the context reading takes the error token — the caller
/// picks the ink, and the row paints it on both the label and the value.
#[test]
fn infoline_context_takes_the_error_token_at_eighty() {
    let theme = &UI_THEME;
    let failure = crate::palette::grammar::chrome_style(theme, ChromeInk::Failure)
        .fg
        .expect("failure ink has a colour");
    for (pct, expect_failure) in [(79u8, false), (80, true), (99, true)] {
        let segments = vec![
            InfoSegment::new(InfoSegmentId::Model, "", "deepseek-v4", ChromeInk::Identity),
            context(pct),
        ];
        let buf = render_buffer(theme, 80, &segments);
        let row = render_row(theme, 80, &segments);
        let start = row.find("ctx").expect("context reading painted");
        let value_fg = buf[(u16::try_from(start + 4).unwrap(), 0)].fg;
        let label_fg = buf[(u16::try_from(start).unwrap(), 0)].fg;
        assert_eq!(value_fg == failure, expect_failure, "{pct}%: value ink");
        assert_eq!(label_fg == failure, expect_failure, "{pct}%: label ink");
    }
}

/// The hint must name a chord that actually opens help in this shell. `F1`
/// is eaten by tmux and several emulators, and bare `?` is composer text.
#[test]
fn infoline_help_hint_names_a_chord_that_opens_help() {
    let hint = help_hint();
    assert!(hint.ends_with(" help"), "{hint}");
    assert!(!hint.contains("F1"), "terminals eat F1: {hint}");
    assert!(!hint.starts_with('?'), "bare ? is composer text: {hint}");
    let chord = hint.split_whitespace().next().unwrap();
    let key = crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('/'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    assert_eq!(chord, "Ctrl+/");
    assert!(crate::tui::shell_key_routing::is_help_shortcut(&key));
    let row = render_row(&UI_THEME, 120, &work_segments());
    assert!(row.trim_end().ends_with(&hint), "pinned right: {row:?}");
}

/// Every recorded hitbox covers exactly the cells its segment painted, at
/// every width — the hitbox pass and the paint pass share one shed pass.
#[test]
fn infoline_hitboxes_match_painted_cells() {
    let segments = work_segments();
    let hint = help_hint();
    for w in 24..=180u16 {
        let area = Rect::new(0, 0, w, 1);
        let info = InfoLine::new(&UI_THEME, &hint, &segments);
        let hitboxes = infoline_hitboxes(&info, area);
        let cells = render_cells(&UI_THEME, w, &segments);
        for hitbox in &hitboxes {
            let segment = segments.iter().find(|s| s.id == hitbox.id).unwrap();
            let painted: String = cells
                [usize::from(hitbox.area.x)..usize::from(hitbox.area.x + hitbox.area.width)]
                .concat();
            let expected = if segment.label.is_empty() {
                segment.value.clone()
            } else {
                format!("{} {}", segment.label, segment.value)
            };
            assert!(
                expected.starts_with(painted.trim_end()),
                "{w}: {:?} hitbox {:?} covers {painted:?}, expected {expected:?}",
                hitbox.id,
                hitbox.area
            );
        }
        // No two hitboxes overlap.
        for (i, a) in hitboxes.iter().enumerate() {
            for b in &hitboxes[i + 1..] {
                assert!(
                    a.area.right() <= b.area.x || b.area.right() <= a.area.x,
                    "{w}: hitboxes overlap: {a:?} {b:?}"
                );
            }
        }
    }
}

/// The context reading's hitbox is exactly the painted `ctx NN%` span.
#[test]
fn context_meter_hitbox_covers_exactly_the_painted_reading() {
    let segments = work_segments();
    let hint = help_hint();
    for w in 24..=180u16 {
        let area = Rect::new(0, 0, w, 1);
        let info = InfoLine::new(&UI_THEME, &hint, &segments);
        let hitbox = context_meter_hitbox(&info, area).expect("the reading never sheds");
        let cells = render_cells(&UI_THEME, w, &segments);
        let painted: String =
            cells[usize::from(hitbox.x)..usize::from(hitbox.x + hitbox.width)].concat();
        assert!(
            "ctx 61%".starts_with(painted.trim_end()),
            "{w}: context hitbox {hitbox:?} covers {painted:?}"
        );
    }
}

/// ASCII-safe mode projects every glyph to a single-width ASCII cell.
#[test]
fn infoline_ascii_safe_has_no_wide_or_unsupported_glyphs() {
    let segments = work_segments();
    let hint = help_hint();
    for (w, _) in BLOCKER_SIZES {
        let area = Rect::new(0, 0, w, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let info = InfoLine::new(&UI_THEME, &hint, &segments).ascii_safe(true);
        ratatui::widgets::Widget::render(info, area, &mut buf);
        for x in 0..w {
            let symbol = buf[(x, 0)].symbol();
            assert!(symbol.is_ascii(), "{w}: cell {x} {symbol:?} is not ASCII");
            assert_eq!(
                symbol.width(),
                1,
                "{w}: cell {x} {symbol:?} is not one cell"
            );
        }
    }
}

/// Hover and degenerate sizes never panic, and hover only brightens the
/// model — the one segment with an action.
#[test]
fn infoline_hover_and_narrow_do_not_panic() {
    let segments = work_segments();
    let hint = help_hint();
    for (w, h) in [(0u16, 0u16), (1, 1), (5, 1), (24, 1), (300, 1)] {
        let area = Rect::new(0, 0, w, h);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let info = InfoLine::new(&UI_THEME, &hint, &segments).hovered(Some(InfoSegmentId::Model));
        ratatui::widgets::Widget::render(info, area, &mut buf);
        let info = InfoLine::new(&UI_THEME, &hint, &segments);
        let _ = infoline_hitboxes(&info, area);
        let _ = context_meter_hitbox(&info, area);
    }
    let area = Rect::new(0, 0, 120, 1);
    let mut plain = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(InfoLine::new(&UI_THEME, &hint, &segments), area, &mut plain);
    let mut hovered = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(
        InfoLine::new(&UI_THEME, &hint, &segments).hovered(Some(InfoSegmentId::Model)),
        area,
        &mut hovered,
    );
    assert_ne!(plain[(0, 0)].modifier, hovered[(0, 0)].modifier);
    let ctx_x = u16::try_from(render_row(&UI_THEME, 120, &segments).find("ctx").unwrap()).unwrap();
    assert_eq!(plain[(ctx_x, 0)], hovered[(ctx_x, 0)]);
}

/// Slice G: the context reading owns the inspector click action, so it
/// brightens on hover exactly like the model segment; status-only facts
/// (cost) never do.
#[test]
fn infoline_context_hover_brightens_only_the_context_reading() {
    let segments = work_segments();
    let hint = help_hint();
    let area = Rect::new(0, 0, 120, 1);
    let mut plain = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(InfoLine::new(&UI_THEME, &hint, &segments), area, &mut plain);
    let mut hovered = ratatui::buffer::Buffer::empty(area);
    ratatui::widgets::Widget::render(
        InfoLine::new(&UI_THEME, &hint, &segments).hovered(Some(InfoSegmentId::Context)),
        area,
        &mut hovered,
    );
    let row = render_row(&UI_THEME, 120, &segments);
    // Hover feedback lands on the value cells (`61%`); the dim label prefix
    // (`ctx`) keeps its reading ink, mirroring the model segment's probe.
    let ctx_x = u16::try_from(row.find("61%").unwrap()).unwrap();
    assert_ne!(
        plain[(ctx_x, 0)].modifier,
        hovered[(ctx_x, 0)].modifier,
        "hovered context reading must respond visibly"
    );
    // Model (actionable but not hovered) and cost (status-only) stay clean.
    assert_eq!(plain[(0, 0)], hovered[(0, 0)]);
    let cost_x = u16::try_from(row.find("$0.42").unwrap()).unwrap();
    assert_eq!(plain[(cost_x, 0)], hovered[(cost_x, 0)]);
}
