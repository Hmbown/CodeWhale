//! Golden-buffer contract for the shell's info line (spec §5c/§6).
//!
//! Each golden is a cell-exact `.txt` dump of the rendered row at one of the
//! four canonical blocker sizes (`views/status_picker.rs::BLOCKER_SIZES`).
//! The goldens are the design contract: exact characters, exact columns.
//!
//! Re-bless after an intentional design change by DELETING the golden file
//! and running:
//!
//! ```sh
//! CODEWHALE_BLESS_GOLDENS=1 ./scripts/dev-test.sh tui infoline
//! ```

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::{InfoLine, InfoSegment, InfoSegmentId};
use crate::palette::{ChromeInk, UI_THEME, UiTheme};

/// The hint the live shell advertises, from the one binding module that owns
/// it — a fixture string here would let chrome and routing drift apart.
fn help_hint() -> String {
    crate::tui::shell_key_routing::info_help_hint(crate::localization::Locale::En)
}

fn context_label() -> String {
    crate::localization::tr(
        crate::localization::Locale::En,
        crate::localization::MessageId::FooterHintContext,
    )
    .into_owned()
}

const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

/// Approved startup screen: no route yet, the workspace names what the
/// session opened.
fn startup_segments() -> Vec<InfoSegment> {
    vec![
        InfoSegment::new(
            InfoSegmentId::Workspace,
            "",
            "codewhale",
            ChromeInk::Metadata,
        ),
        InfoSegment::new(
            InfoSegmentId::Model,
            "",
            "model not connected",
            ChromeInk::Waiting,
        ),
    ]
}

/// Approved work screen: repository, branch, effective model. The repository
/// segment states the forge slug and keeps the folder basename as its shorter
/// form.
fn work_segments() -> Vec<InfoSegment> {
    vec![
        InfoSegment::new(
            InfoSegmentId::Workspace,
            "",
            "Hmbown/CodeWhale",
            ChromeInk::Metadata,
        )
        .short("codewhale"),
        InfoSegment::new(InfoSegmentId::Branch, "⑂", "main", ChromeInk::Metadata),
        InfoSegment::new(InfoSegmentId::Model, "", "deepseek-v4", ChromeInk::Identity),
    ]
}

/// Approved settings screen: breadcrumb, repository, effective model.
fn settings_segments() -> Vec<InfoSegment> {
    vec![
        InfoSegment::new(
            InfoSegmentId::SettingsPath,
            "",
            "Settings / Appearance",
            ChromeInk::Identity,
        ),
        InfoSegment::new(
            InfoSegmentId::Workspace,
            "",
            "codewhale",
            ChromeInk::Metadata,
        ),
        InfoSegment::new(
            InfoSegmentId::Model,
            "",
            "claude-3.5-sonnet",
            ChromeInk::Identity,
        ),
    ]
}

/// The work fixture plus the conditional work facts the live shell adds when
/// a run, a pod, or scheduled automation is live. Used by the shed test: the
/// declared order has to hold with the whole ladder present.
fn crowded_segments() -> Vec<InfoSegment> {
    let mut segments = work_segments();
    // A slug whose two forms share no substring, so the shed sweep can tell
    // "slug" from "basename" from "segment gone".
    segments[0] = InfoSegment::new(
        InfoSegmentId::Workspace,
        "",
        "acme/mcp-gateway",
        ChromeInk::Metadata,
    )
    .short("mcp-gateway");
    segments.insert(
        2,
        InfoSegment::new(InfoSegmentId::Run, "run", "release 0.9.12", ChromeInk::Info),
    );
    segments.insert(
        3,
        InfoSegment::new(InfoSegmentId::Pod, "pod", "launch pod", ChromeInk::Active),
    );
    segments.insert(
        4,
        InfoSegment::new(InfoSegmentId::Whales, "whales", "3/4", ChromeInk::Info),
    );
    segments
}

fn fixtures() -> Vec<(&'static str, Vec<InfoSegment>, u8)> {
    vec![
        ("startup", startup_segments(), 0),
        ("work", work_segments(), 61),
        ("settings", settings_segments(), 61),
    ]
}

fn render_buffer(
    theme: &UiTheme,
    width: u16,
    segments: &[InfoSegment],
    pct: u8,
) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(width, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    let hint = help_hint();
    terminal
        .draw(|frame| {
            let context = context_label();
            let info = InfoLine::new(theme, &hint, &context, pct, segments);
            use ratatui::widgets::Widget;
            Widget::render(info, frame.area(), frame.buffer_mut());
        })
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn render_row(theme: &UiTheme, width: u16, segments: &[InfoSegment], pct: u8) -> String {
    render_cells(theme, width, segments, pct).concat()
}

/// Per-cell symbols of one rendered row (the golden dump, before joining).
fn render_cells(theme: &UiTheme, width: u16, segments: &[InfoSegment], pct: u8) -> Vec<String> {
    render_buffer(theme, width, segments, pct)
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
    for (screen, segments, pct) in fixtures() {
        for (w, h) in BLOCKER_SIZES {
            let name = format!("infoline_{screen}_{w}x{h}");
            let rendered = render_row(&UI_THEME, w, &segments, pct);
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

/// The row states no time of day, and carries no wordmark: the mark belongs
/// to the launch header and nowhere else in the default look (§2.0).
#[test]
fn infoline_states_no_clock_and_no_wordmark() {
    for (_, segments, pct) in fixtures() {
        for (w, _h) in BLOCKER_SIZES {
            let row = render_row(&UI_THEME, w, &segments, pct);
            assert!(
                !row.contains(':'),
                "{w}: the info line carries no clock: {row:?}"
            );
            assert!(
                !row.contains("CODEWHALE"),
                "{w}: no wordmark on this row: {row:?}"
            );
        }
    }
    // The work fixture's own repository is the only `codewhale` on the row,
    // and only once it has shed the slug.
    let work = render_row(&UI_THEME, 160, &work_segments(), 61);
    assert!(!work.contains("codewhale"), "no wordmark: {work:?}");
}

/// Declared shed order (spec §5b): the bar glyphs, then the help hint, then
/// the repository slug down to the folder basename, then folder, then
/// branch. The route identity and the `context NN%` text are the floor at
/// every width.
#[test]
fn infoline_sheds_bar_then_help_then_slug_then_folder_then_branch() {
    let segments = crowded_segments();
    // The narrowest row that still shows a thing. A thing that sheds earlier
    // needs a wider row to survive, so these strictly decrease down the
    // declared order.
    let narrowest_showing = |needle: &str| -> u16 {
        (24..=180u16)
            .filter(|w| render_row(&UI_THEME, *w, &segments, 61).contains(needle))
            .min()
            .unwrap_or_else(|| panic!("{needle} never painted at any width"))
    };
    let bar = narrowest_showing("▱");
    let help = narrowest_showing("help");
    let slug = narrowest_showing("acme/");
    let folder = narrowest_showing("mcp-gateway");
    let branch = narrowest_showing("⑂ main");
    assert!(
        bar > help && help > slug && slug > folder && folder > branch,
        "shed order drifted: bar {bar}, help {help}, slug {slug}, \
         folder {folder}, branch {branch}"
    );
    // The slug degrades to the basename rather than costing the row a whole
    // segment: the repository is still named at every width the folder
    // survives.
    for width in folder..=180u16 {
        let row = render_row(&UI_THEME, width, &segments, 61);
        assert!(
            row.contains("mcp-gateway"),
            "{width}: the repository stays named: {row:?}"
        );
    }
    // What 80 columns spend their cells on: the repository under its real
    // name, the branch, the model, the reading, and the hint. Dropping the
    // top bar's wordmark and the `model` label bought exactly this.
    let row80 = render_row(&UI_THEME, 80, &work_segments(), 61);
    for expected in ["Hmbown/CodeWhale", "⑂ main", "deepseek-v4", "context 61%"] {
        assert!(
            row80.contains(expected),
            "80: {expected} must survive: {row80:?}"
        );
    }
    assert!(
        row80.contains("help"),
        "80: the hint survives too: {row80:?}"
    );

    // Below the floor (route identity + join + reading = 25 cells for this
    // fixture) the reading pins right, whole, and the route is what yields.
    let row24 = render_row(&UI_THEME, 24, &segments, 61);
    assert!(
        row24.trim_end().ends_with("context 61%"),
        "24: below the floor the reading pins right: {row24:?}"
    );
    assert!(
        row24.starts_with("deepseek"),
        "24: the route keeps the left edge: {row24:?}"
    );
    for width in 24..=180u16 {
        let row = render_row(&UI_THEME, width, &segments, 61);
        assert!(
            row.contains("context 61%"),
            "{width}: the context reading is the floor: {row:?}"
        );
        if width >= 60 {
            assert!(
                row.contains("deepseek-v4"),
                "{width}: route identity never sheds first: {row:?}"
            );
        }
    }
}

/// The bar is a solid 10-cell reading: filled cells are the tenths used.
#[test]
fn infoline_bar_fills_one_cell_per_tenth() {
    for (pct, filled) in [(0u8, 0usize), (61, 6), (80, 8), (100, 10)] {
        let row = render_row(&UI_THEME, 160, &work_segments(), pct);
        assert!(
            row.contains(&format!("context {pct}%")),
            "{pct}: the reading is the number: {row:?}"
        );
        assert_eq!(
            row.matches('▰').count(),
            filled,
            "{pct}% must fill {filled} of 10 cells: {row:?}"
        );
        assert_eq!(
            row.matches('▱').count(),
            10 - filled,
            "{pct}% must leave {} cells open: {row:?}",
            10 - filled
        );
    }
}

/// At the 80% cap the whole context reading turns to the error token — the
/// number, the percent sign, and the bar, so it reads as one warning.
#[test]
fn infoline_context_takes_the_error_token_at_eighty() {
    let segments = work_segments();
    let warn = render_buffer(&UI_THEME, 160, &segments, 83);
    let calm = render_buffer(&UI_THEME, 160, &segments, 61);
    let fg_of = |buf: &ratatui::buffer::Buffer, needle: char| {
        (0..160u16)
            .find(|x| buf[(*x, 0)].symbol() == needle.to_string())
            .map(|x| buf[(x, 0)].fg)
            .expect("row paints the glyph")
    };
    assert_eq!(fg_of(&warn, '%'), UI_THEME.error_fg, "83% is the error ink");
    assert_eq!(fg_of(&warn, '▰'), UI_THEME.error_fg, "the bar warns too");
    assert_ne!(
        fg_of(&calm, '%'),
        UI_THEME.error_fg,
        "61% is a status, not a failure"
    );
    assert_eq!(super::meter_ink_for(83), ChromeInk::Failure);
    assert_eq!(super::meter_ink_for(79), ChromeInk::Info);
    assert_eq!(super::context_label_ink_for(83), ChromeInk::Failure);
    assert_eq!(super::context_label_ink_for(79), ChromeInk::Metadata);
}

/// The repository segment states `owner/name` while the row can afford it,
/// and falls back to the folder basename — never to nothing — when it
/// cannot. A shorter form is taken before any segment goes.
#[test]
fn infoline_repository_slug_falls_back_to_the_folder_basename() {
    let segments = work_segments();
    let wide = render_row(&UI_THEME, 120, &segments, 61);
    assert!(
        wide.contains("Hmbown/CodeWhale"),
        "the slug is the repository's name when it fits: {wide:?}"
    );
    let tight = render_row(&UI_THEME, 52, &segments, 61);
    assert!(
        !tight.contains("Hmbown/CodeWhale"),
        "52 cannot afford the slug: {tight:?}"
    );
    assert!(
        tight.contains("codewhale"),
        "the basename keeps the slot: {tight:?}"
    );
    // A "shorter" form that is not shorter is not adopted.
    let no_short = InfoSegment::new(InfoSegmentId::Workspace, "", "cw", ChromeInk::Metadata)
        .short("a-much-longer-name");
    assert!(no_short.short.is_none());
}

/// The hint must name a chord that actually opens help in this shell. `F1`
/// is advertised nowhere in chrome because terminals eat it, and bare `?` is
/// composer text; `Ctrl+/` is what `is_help_shortcut` accepts unconditionally.
#[test]
fn infoline_help_hint_names_a_chord_that_opens_help() {
    use crate::tui::shell_key_routing::{HELP_CHROME_CHORD, info_help_hint, is_help_shortcut};
    assert_eq!(HELP_CHROME_CHORD, "Ctrl+/");
    assert!(
        is_help_shortcut(&KeyEvent::new(KeyCode::Char('/'), KeyModifiers::CONTROL)),
        "the advertised chord must open help"
    );
    // The chords chrome deliberately does not advertise.
    assert!(
        !is_help_shortcut(&KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE)),
        "bare ? types text, so it must never be the printed hint"
    );
    let hint = info_help_hint(crate::localization::Locale::En);
    assert!(!hint.contains("F1"), "terminals eat F1: {hint}");
    let row = render_row(&UI_THEME, 160, &work_segments(), 61);
    assert!(row.ends_with(&hint), "the hint is pinned right: {row:?}");
}

#[test]
fn infoline_hitboxes_match_painted_cells() {
    use super::infoline_hitboxes;
    let segments = startup_segments();
    let hint = help_hint();
    let context = context_label();
    let info = InfoLine::new(&UI_THEME, &hint, &context, 0, &segments);
    let area = ratatui::layout::Rect::new(0, 0, 160, 1);
    let hitboxes = infoline_hitboxes(&info, area);
    assert_eq!(hitboxes.len(), 2, "one hitbox per painted segment");
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
    // cell, not by byte: the row contains multi-byte glyphs (`·`, meter).
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

#[test]
fn infoline_hitboxes_follow_the_same_shed_pass_as_paint() {
    let segments = crowded_segments();
    let hint = help_hint();
    for width in [120u16, 80, 60, 44, 30, 20] {
        let context = context_label();
        let info = InfoLine::new(&UI_THEME, &hint, &context, 61, &segments);
        let hitboxes = super::infoline_hitboxes(&info, ratatui::layout::Rect::new(0, 0, width, 1));
        let cells = render_cells(&UI_THEME, width, &segments, 61);
        for hitbox in hitboxes {
            assert!(
                hitbox.area.right() <= width,
                "{width}: hitbox {:?} escapes the row: {:?}",
                hitbox.id,
                hitbox.area
            );
            let text: String = (hitbox.area.x..hitbox.area.right())
                .filter_map(|x| cells.get(usize::from(x)))
                .cloned()
                .collect();
            assert!(
                !text.trim().is_empty(),
                "{width}: hitbox {:?} covers unpainted cells",
                hitbox.id
            );
        }
    }

    let context = context_label();
    let segments = work_segments();
    for width in 20..=24 {
        let info = InfoLine::new(&UI_THEME, &hint, &context, 61, &segments);
        let area = ratatui::layout::Rect::new(0, 0, width, 1);
        let model = super::infoline_hitboxes(&info, area)
            .into_iter()
            .find(|hitbox| hitbox.id == InfoSegmentId::Model);
        let context = super::context_meter_hitbox(&info, area).expect("context hitbox");
        assert!(
            model.is_none_or(|model| {
                model.area.right() <= context.x || context.right() <= model.area.x
            }),
            "{width}: model and context hitboxes overlap"
        );
    }
}

#[test]
fn infoline_ascii_safe_has_no_wide_or_unsupported_glyphs() {
    let segments = work_segments();
    let hint = help_hint();
    let row = {
        let backend = TestBackend::new(160, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let context = context_label();
                let info =
                    InfoLine::new(&UI_THEME, &hint, &context, 61, &segments).ascii_safe(true);
                use ratatui::widgets::Widget;
                Widget::render(info, frame.area(), frame.buffer_mut());
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
    assert!(row.contains('#'), "meter projects to #");
    assert!(!row.contains('▰'), "no block glyphs survive ascii-safe");
    assert!(!row.contains('⑂'), "the branch glyph projects too: {row}");
    for ch in row.chars() {
        assert_eq!(ch.width(), Some(1), "ascii-safe row must be single-width");
    }
}

#[test]
fn infoline_hover_and_narrow_do_not_panic() {
    let segments = work_segments();
    let hint = help_hint();
    // Hover style change must not move cells.
    let plain = render_row(&UI_THEME, 160, &segments, 61);
    let hovered = {
        let backend = TestBackend::new(160, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let context = context_label();
                let info = InfoLine::new(&UI_THEME, &hint, &context, 61, &segments)
                    .hovered(Some(InfoSegmentId::Model));
                use ratatui::widgets::Widget;
                Widget::render(info, frame.area(), frame.buffer_mut());
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
    assert_eq!(plain, hovered, "hover recolors, it does not relayout");
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
    // element paints), proven against the buffer itself at row widths that
    // are roomy (nothing sheds), tight (help and segments shed), and too
    // narrow (no hitbox rather than an overlapping one).
    let segments = crowded_segments();
    let hint = help_hint();
    for width in [160u16, 80, 60, 44, 30, 20] {
        let context = context_label();
        let info = InfoLine::new(&UI_THEME, &hint, &context, 61, &segments);
        let row = render_row(&UI_THEME, width, &segments, 61);
        let area = ratatui::layout::Rect::new(0, 0, width, 1);
        match super::context_meter_hitbox(&info, area) {
            Some(hitbox) => {
                let covered = row
                    .chars()
                    .skip(usize::from(hitbox.x))
                    .take(usize::from(hitbox.width))
                    .collect::<String>();
                assert!(
                    covered.starts_with("context "),
                    "{width} wide: hitbox must start at the meter's first cell: {covered:?}"
                );
                assert!(
                    covered.contains('%'),
                    "{width} wide: hitbox must cover the percentage: {covered:?}"
                );
                assert!(
                    !covered.contains("help"),
                    "{width} wide: hitbox must not reach the help hint: {covered:?}"
                );
            }
            None => {
                // Refused only when even the floor cannot fit: the route
                // identity, a join, and the `context NN%` text.
                let floor = "deepseek-v4".width() + " · ".width() + "context 61%".width();
                assert!(
                    usize::from(width) < floor,
                    "{width} wide: a fittable meter (floor {floor}) must return a hitbox"
                );
            }
        }
    }
}
