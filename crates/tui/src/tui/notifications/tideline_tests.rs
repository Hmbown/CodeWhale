//! Golden-buffer contract for the Tideline notifications inbox (spec §5a/
//! §5c). Goldens: `notifications_{w}x{h}` at the four blocker sizes.
//! Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    NotificationCenterView, NotificationKind, NotificationPayload, TidelineInbox,
    TidelineInboxRecord, render_tideline_inbox, tideline_inbox_hitboxes,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};

fn record(kind: NotificationKind, title: &str, at: &str, read: bool) -> TidelineInboxRecord {
    TidelineInboxRecord {
        kind,
        title: title.to_string(),
        body: None,
        at: at.to_string(),
        read,
    }
}

/// The approved inbox fixture: one interactive ask, one completion, one read
/// terminal whale, second row selected (so its body row shows).
fn records() -> Vec<TidelineInboxRecord> {
    vec![
        TidelineInboxRecord {
            kind: NotificationKind::ApprovalNeeded,
            title: "rm -rf target/ in worktree".to_string(),
            body: Some("whale-2 wants to clean the build directory".to_string()),
            at: "14:41".to_string(),
            read: false,
        },
        record(
            NotificationKind::TurnComplete,
            "turn surfaced ✓",
            "14:38",
            false,
        ),
        record(
            NotificationKind::SubagentTerminal,
            "whale-3 done",
            "14:20",
            true,
        ),
    ]
}

fn draw(width: u16, height: u16, inbox: &TidelineInbox<'_>) -> String {
    render_golden_text(width, height, |buf| {
        render_tideline_inbox(Rect::new(0, 0, width, height), buf, inbox);
    })
}

#[test]
fn notifications_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        let recs = records();
        let inbox = TidelineInbox::new(&UI_THEME, &recs).selected(0);
        assert_matches_golden(&format!("notifications_{w}x{h}"), &draw(w, h, &inbox));
    }
}

#[test]
fn notifications_header_counts_unread() {
    let recs = records();
    let inbox = TidelineInbox::new(&UI_THEME, &recs);
    let text = draw(80, 24, &inbox);
    assert!(text.contains("NOTIFICATIONS · 2 unread"), "{text}");
    let all_read: Vec<TidelineInboxRecord> = recs
        .iter()
        .map(|r| TidelineInboxRecord {
            read: true,
            ..r.clone()
        })
        .collect();
    let text = draw(80, 24, &TidelineInbox::new(&UI_THEME, &all_read));
    let header = text.lines().next().unwrap_or_default().trim();
    assert_eq!(header, "NOTIFICATIONS", "read inbox drops the count");
}

#[test]
fn notifications_unread_rows_carry_the_gold_mark() {
    let recs = records();
    let inbox = TidelineInbox::new(&UI_THEME, &recs);
    let text = draw(80, 24, &inbox);
    assert!(
        text.contains("◆ approval"),
        "unread ask is gold-marked: {text}"
    );
    assert!(text.contains("○ whale done"), "read row is hollow: {text}");
}

#[test]
fn notifications_selected_body_replaces_not_doubles() {
    let recs = records();
    let inbox = TidelineInbox::new(&UI_THEME, &recs).selected(0);
    let text = draw(80, 24, &inbox);
    assert!(
        text.contains("whale-2 wants to clean"),
        "selected body row: {text}"
    );
    // Row 2 (the completion) must still be present exactly once.
    assert_eq!(text.matches("turn surfaced").count(), 1, "{text}");
}

#[test]
fn notifications_empty_state_is_quiet_not_blank() {
    let inbox = TidelineInbox::new(&UI_THEME, &[]);
    let text = draw(80, 24, &inbox);
    assert!(text.contains("quiet water"), "{text}");
}

#[test]
fn notifications_ascii_safe_projects_marks() {
    let recs = records();
    let inbox = TidelineInbox::new(&UI_THEME, &recs).ascii_safe(true);
    let text = draw(80, 24, &inbox);
    assert!(text.contains("* approval"), "gold ◆ projects to *: {text}");
    assert!(
        text.contains(". whale done"),
        "read ○ projects to .: {text}"
    );
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn notifications_hitboxes_match_painted_rows() {
    let recs = records();
    let inbox = TidelineInbox::new(&UI_THEME, &recs).selected(0);
    let (w, h) = (100, 30);
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    render_tideline_inbox(area, &mut buf, &inbox);
    let hitboxes = tideline_inbox_hitboxes(area, &inbox);
    assert_eq!(hitboxes.len(), 3, "one rect per record");
    // First rect covers the selected record's body row.
    assert_eq!(hitboxes[0].height, 2);
    // No overlaps, all inside, all cover painted text.
    for pair in hitboxes.windows(2) {
        assert!(pair[0].y + pair[0].height <= pair[1].y, "no overlap");
    }
    for rect in &hitboxes {
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(!cells.trim().is_empty(), "rect {rect:?} covers empty cells");
    }
}

#[test]
fn notifications_kind_words_never_say_error() {
    for kind in [
        NotificationKind::TurnComplete,
        NotificationKind::SubagentTerminal,
        NotificationKind::ApprovalNeeded,
        NotificationKind::InputNeeded,
        NotificationKind::ElevationNeeded,
        NotificationKind::ModelNotify,
    ] {
        let rec = record(kind, "t", "00:00", false);
        assert_ne!(rec.kind_word(), "error");
        assert_ne!(rec.kind_word(), "Error");
    }
}

#[test]
fn notifications_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (4, 1), (8, 2), (200, 50)] {
        let recs = records();
        let inbox = TidelineInbox::new(&UI_THEME, &recs);
        let _ = draw(w, h, &inbox);
    }
}

fn inbox_test_app() -> crate::tui::app::App {
    crate::test_support::test_app_with_options(crate::test_support::test_tui_options("."))
}

#[test]
fn notification_center_projects_records_with_the_read_watermark() {
    let mut app = inbox_test_app();
    app.record_notification_payload(&NotificationPayload::approval_needed(
        "Approval needed",
        "bash",
    ));
    app.record_notification_payload(&NotificationPayload::turn_complete("Turn complete"));

    let view = NotificationCenterView::new(&app);
    assert_eq!(view.records.len(), 2, "one row per retained record");
    assert!(
        view.records.iter().all(|record| !record.read),
        "fresh records are unread"
    );

    // Marking read advances the watermark; the records stay inspectable.
    app.mark_notifications_read();
    let mut view = NotificationCenterView::new(&app);
    assert!(
        view.records.iter().all(|record| record.read),
        "the watermark reads every retained record"
    );

    // A later ask marks itself unread again without resurrecting the old.
    app.record_notification_payload(&NotificationPayload::input_needed("Input needed"));
    view.refresh_from_app(&app);
    assert_eq!(view.records.len(), 3);
    assert!(!view.records[2].read, "the new ask is unread");
    assert!(view.records[0].read && view.records[1].read);
}

#[test]
fn notification_retention_stays_bounded() {
    let mut app = inbox_test_app();
    for index in 0..30 {
        app.record_notification_payload(&NotificationPayload::turn_complete(&format!(
            "Turn complete {index}"
        )));
    }
    assert_eq!(
        app.notification_records.len(),
        super::INBOX_RECORDS_MAX,
        "the inbox keeps only the newest records"
    );
    assert_eq!(
        app.notification_records
            .last()
            .map(|record| record.title.as_str()),
        Some("Turn complete 29"),
        "the newest record survives the bound"
    );
}
