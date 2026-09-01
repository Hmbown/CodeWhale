//! Shared golden-buffer harness for the Tideline components (spec §5c).
//!
//! Every Tideline component proves itself against cell-exact golden buffers
//! at the four canonical blocker sizes. This module owns the one dump format
//! and the one bless protocol so they cannot drift between components:
//!
//! - rows are the `Buffer` cell symbols in paint order, `width` per row;
//! - rows are joined by `\n` with one trailing newline;
//! - goldens live in `crates/tui/src/tui/goldens/{name}_{w}x{h}.txt`;
//! - a missing golden fails the test unless `CODEWHALE_BLESS_GOLDENS=1` is
//!   set, in which case the rendered text is written as the new contract.
//!
//! Goldens are the design contract — a visual change that cannot show as a
//! golden diff did not happen.

/// The four terminal sizes the v0.8.66 modal blocker (#3732) requires every
/// surface to remain readable and fully operable at. Mirrors
/// `views/status_picker.rs::BLOCKER_SIZES` (kept private there, so the
/// canonical copy is restated here for the Tideline golden suites).
pub(crate) const BLOCKER_SIZES: [(u16, u16); 4] = [(80, 24), (100, 30), (120, 32), (160, 40)];

/// Render one component into a fresh buffer and dump the cell symbols as
/// golden text. Deterministic by contract: the caller injects every fact
/// (clock strings, counters, hover state), never `Instant::now`.
pub(crate) fn render_golden_text(
    width: u16,
    height: u16,
    draw: impl FnOnce(&mut ratatui::buffer::Buffer),
) -> String {
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(
        0,
        0,
        width.max(1),
        height.max(1),
    ));
    draw(&mut buf);
    let w = width.max(1) as usize;
    let content = buf.content();
    let rows: Vec<String> = (0..height.max(1) as usize)
        .map(|y| {
            content[y * w..(y + 1) * w]
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect()
        })
        .collect();
    format!("{}\n", rows.join("\n"))
}

/// Assert `rendered` equals the golden `name`, blessing it when missing and
/// `CODEWHALE_BLESS_GOLDENS=1` is set (topbar protocol, spec §5c).
pub(crate) fn assert_matches_golden(name: &str, rendered: &str) {
    let path = golden_path(name);
    match std::fs::read_to_string(&path) {
        // Compare against LF: a Windows checkout can hand us CRLF, and the
        // dump side always joins rows with LF. Cell symbols never contain CR,
        // so this can only ever cancel a line-ending difference.
        Ok(expected) => assert_eq!(
            rendered,
            expected.replace("\r\n", "\n"),
            "golden drift at {name}; re-bless only with an approved design change"
        ),
        Err(_) => {
            if std::env::var("CODEWHALE_BLESS_GOLDENS").is_ok() {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).expect("create goldens dir");
                }
                std::fs::write(&path, rendered).expect("write golden");
            } else {
                panic!("missing golden {name}; run with CODEWHALE_BLESS_GOLDENS=1 to write it");
            }
        }
    }
}

pub(crate) fn golden_path(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tui/goldens")
        .join(format!("{name}.txt"))
}

// ---------------------------------------------------------------------------
// Ink plane — the colour half of the design contract.
//
// `render_golden_text` dumps cell *symbols*. That is only half a render: a
// screen can go from a gold mark over a blue gradient to uniform grey without
// moving a single glyph, and the symbol golden would not notice. The founder's
// standing complaint about the startup screen ("everything in the same dim
// gray") was, mechanically, invisible to this suite.
//
// The ink plane closes that hole. Each painted cell becomes one character
// keyed to its (fg, bg, modifier) triple, with a legend resolving those keys
// to concrete values, so a contrast change shows up as a golden diff.
// ---------------------------------------------------------------------------

/// Stable key alphabet for the ink plane. Keys are assigned in paint order
/// (row-major), so a legend diff reads as "what changed, where it first
/// appears" rather than as an unrelated reshuffle.
const INK_KEYS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// One distinct paint style seen in a rendered buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InkEntry {
    pub key: char,
    /// Human-readable style, e.g. `#F6C453 on reset BOLD`.
    pub description: String,
    /// How many cells carry this style.
    pub cells: usize,
    /// How many *non-blank* cells carry it (blank = space symbol).
    pub glyph_cells: usize,
}

/// Render once and describe every distinct paint style in the buffer.
///
/// Deterministic by the same contract as [`render_golden_text`]: the caller
/// injects every fact, so the entries and their order are reproducible.
pub(crate) fn render_ink_entries(
    width: u16,
    height: u16,
    draw: impl FnOnce(&mut ratatui::buffer::Buffer),
) -> (String, Vec<InkEntry>) {
    let (w, h) = (width.max(1), height.max(1));
    let mut buf = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(0, 0, w, h));
    draw(&mut buf);

    let mut order: Vec<(
        ratatui::style::Color,
        ratatui::style::Color,
        ratatui::style::Modifier,
    )> = Vec::new();
    let mut entries: Vec<InkEntry> = Vec::new();
    let mut plane = String::with_capacity(usize::from(w) * usize::from(h) + usize::from(h));

    let content = buf.content();
    for y in 0..usize::from(h) {
        for x in 0..usize::from(w) {
            let cell = &content[y * usize::from(w) + x];
            let triple = (cell.fg, cell.bg, cell.modifier);
            let index = match order.iter().position(|seen| *seen == triple) {
                Some(index) => index,
                None => {
                    order.push(triple);
                    entries.push(InkEntry {
                        // Beyond the alphabet the plane stops being readable;
                        // '?' marks the overflow honestly rather than aliasing
                        // two distinct styles onto one key.
                        key: INK_KEYS
                            .get(order.len() - 1)
                            .map_or('?', |byte| char::from(*byte)),
                        description: describe_ink(triple.0, triple.1, triple.2),
                        cells: 0,
                        glyph_cells: 0,
                    });
                    order.len() - 1
                }
            };
            entries[index].cells += 1;
            if cell.symbol() != " " {
                entries[index].glyph_cells += 1;
            }
            plane.push(entries[index].key);
        }
        plane.push('\n');
    }
    (plane, entries)
}

/// Dump the ink plane and its legend in the golden text format.
pub(crate) fn render_golden_ink(
    width: u16,
    height: u16,
    draw: impl FnOnce(&mut ratatui::buffer::Buffer),
) -> String {
    let (plane, entries) = render_ink_entries(width, height, draw);
    let mut out = plane;
    out.push_str("--\n");
    for entry in &entries {
        out.push_str(&format!("{} {}\n", entry.key, entry.description));
    }
    out
}

fn describe_ink(
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    modifier: ratatui::style::Modifier,
) -> String {
    let mut out = format!("{} on {}", describe_color(fg), describe_color(bg));
    if !modifier.is_empty() {
        // `Modifier`'s Debug is a stable bitflags list (`BOLD | DIM`), which is
        // exactly the shape we want in a legend line.
        out.push_str(&format!(" {modifier:?}"));
    }
    out
}

fn describe_color(value: ratatui::style::Color) -> String {
    match value {
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
        ratatui::style::Color::Reset => "reset".to_string(),
        other => format!("{other:?}").to_lowercase(),
    }
}
