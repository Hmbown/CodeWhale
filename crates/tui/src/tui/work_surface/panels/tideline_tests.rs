//! Golden-buffer contract for the Tideline fleet ledger (spec §2/§5c).
//! Goldens: `ledger_{w}x{h}` at the four blocker sizes — the 80-wide
//! golden proves the three-column shed. Re-bless with
//! `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    TidelineLedgerColumns, TidelineLedgerRow, TidelinePodLedger, TidelineWhaleState,
    render_tideline_ledger, tideline_ledger_hitboxes,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

fn row(
    whale: &str,
    assignment: &str,
    state: TidelineWhaleState,
    elapsed: &str,
    receipts: &str,
    last_update: &str,
) -> TidelineLedgerRow {
    TidelineLedgerRow {
        whale: whale.to_string(),
        assignment: assignment.to_string(),
        state,
        elapsed: elapsed.to_string(),
        receipts: receipts.to_string(),
        last_update: last_update.to_string(),
    }
}

/// The approved work-screen fixture: three whales in every state family.
fn rows() -> Vec<TidelineLedgerRow> {
    vec![
        row(
            "whale-1",
            "restyle the footer band",
            TidelineWhaleState::Working,
            "1m 15s",
            "12",
            "14:42:18",
        ),
        row(
            "whale-2",
            "golden buffers for the ledger",
            TidelineWhaleState::Done,
            "2m 03s",
            "34",
            "14:39:02",
        ),
        row(
            "whale-3",
            "preview render of the work stage",
            TidelineWhaleState::Failed,
            "0m 41s",
            "3",
            "14:20:55",
        ),
    ]
}

fn draw(width: u16, height: u16, ledger: &TidelinePodLedger<'_>) -> String {
    render_golden_text(width, height, |buf| {
        render_tideline_ledger(Rect::new(0, 0, width, height), buf, ledger);
    })
}

#[test]
fn ledger_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        let fixture = rows();
        let ledger = TidelinePodLedger::new(&UI_THEME, &fixture).selected(0);
        assert_matches_golden(&format!("ledger_{w}x{h}"), &draw(w, h, &ledger));
    }
}

#[test]
fn ledger_columns_shed_per_declared_order() {
    assert!(TidelineLedgerColumns::for_width(160).last_update);
    assert!(TidelineLedgerColumns::for_width(130).last_update);
    let at_110 = TidelineLedgerColumns::for_width(110);
    assert!(!at_110.last_update, "LAST UPDATE sheds at 110");
    assert!(at_110.receipts && at_110.elapsed);
    let at_80 = TidelineLedgerColumns::for_width(80);
    assert!(
        !at_80.elapsed && !at_80.receipts && !at_80.last_update,
        "80 cols keeps only WHALE │ ASSIGNMENT │ STATE"
    );
}

#[test]
fn ledger_eighty_columns_is_three_columns() {
    let fixture = rows();
    let ledger = TidelinePodLedger::new(&UI_THEME, &fixture).selected(0);
    let text = draw(80, 24, &ledger);
    let header = text.lines().nth(1).unwrap_or_default();
    assert!(header.contains("WHALE"), "{header}");
    assert!(header.contains("ASSIGNMENT"), "{header}");
    assert!(header.contains("STATE"), "{header}");
    assert!(!header.contains("ELAPSED"), "{header}");
    assert!(!text.contains("14:42:18"), "time sheds at 80: {text}");
}

#[test]
fn ledger_wide_shows_every_column() {
    let fixture = rows();
    let ledger = TidelinePodLedger::new(&UI_THEME, &fixture).selected(0);
    let text = draw(160, 40, &ledger);
    // LAST UPDATE's header truncates to its 8-cell column; the data does not.
    for fact in ["ELAPSED", "RECEIPTS", "LAST UP", "14:42:18", "1m 15s"] {
        assert!(text.contains(fact), "missing {fact} at 160: {text}");
    }
}

#[test]
fn ledger_assignments_truncate_never_wrap() {
    let long = row(
        "whale-9",
        "an assignment objective long enough to overflow any reasonable column budget at eighty columns",
        TidelineWhaleState::Working,
        "0m 05s",
        "1",
        "00:00:05",
    );
    let long_rows = [long];
    let ledger = TidelinePodLedger::new(&UI_THEME, &long_rows);
    let text = draw(80, 24, &ledger);
    let body = text.lines().nth(2).unwrap_or_default();
    assert!(body.contains('…'), "truncation ellipsis: {body}");
    // One line per row — the third line must be blank, not a wrapped tail.
    let third = text.lines().nth(3).unwrap_or_default();
    assert!(third.trim().is_empty(), "no wrap tail: {third:?}");
}

#[test]
fn ledger_state_marks_carry_words() {
    let fixture = rows();
    let ledger = TidelinePodLedger::new(&UI_THEME, &fixture);
    let text = draw(120, 32, &ledger);
    for word in ["● working", "✓ done", "✗ failed"] {
        assert!(text.contains(word), "state mark + word: {text}");
    }
    assert_eq!(
        TidelineWhaleState::Failed.ink(),
        crate::palette::ChromeInk::Failure
    );
}

#[test]
fn ledger_selected_row_marker_and_ascii_safe() {
    let fixture = rows();
    let ledger = TidelinePodLedger::new(&UI_THEME, &fixture)
        .selected(2)
        .ascii_safe(true);
    let text = draw(120, 32, &ledger);
    assert!(text.contains('>'), "selected ▶ projects to >: {text}");
    assert!(text.contains(". working"), "● projects to .: {text}");
    assert!(text.contains("X failed"), "✗ projects to X: {text}");
    assert!(text.contains('|'), "separators project to |: {text}");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn ledger_hitboxes_are_row_rects() {
    let fixture = rows();
    let ledger = TidelinePodLedger::new(&UI_THEME, &fixture).selected(0);
    let (w, h) = (120, 32);
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    render_tideline_ledger(area, &mut buf, &ledger);
    let hitboxes = tideline_ledger_hitboxes(area, &ledger);
    assert_eq!(hitboxes.len(), 3, "one rect per row");
    for (index, rect) in hitboxes.iter().enumerate() {
        assert_eq!(rect.y, 2 + index as u16);
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(
            cells.contains("whale-"),
            "row rect covers its whale: {cells:?}"
        );
    }
}

#[test]
fn ledger_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (10, 1), (29, 2), (30, 2), (300, 50)] {
        let fixture = rows();
        let ledger = TidelinePodLedger::new(&UI_THEME, &fixture);
        let _ = draw(w, h, &ledger);
    }
}
