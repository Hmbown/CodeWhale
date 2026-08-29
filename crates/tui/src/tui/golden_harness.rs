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
