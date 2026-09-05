//! Golden-buffer contract for the Tideline work stage — rail (component 12)
//! + receipt stream (component 8) — spec §5a/§5b/§5c.
//!
//! Goldens: `work_{w}x{h}` at the four blocker sizes (the ledger and
//! composer dock into this stage in the live shell and carry their own
//! suites). Re-bless with `CODEWHALE_BLESS_GOLDENS=1`.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthChar;

use super::{
    TidelineRail, TidelineWorkStage, render_tideline_rail, render_tideline_work_stage,
    tideline_rail_groups, tideline_rail_hitboxes, tideline_rail_width,
};
use crate::palette::UI_THEME;
use crate::tui::golden_harness::{BLOCKER_SIZES, assert_matches_golden, render_golden_text};
use crate::tui::history::tideline_exports::{
    TidelineReceiptState, TidelineStream, TidelineStreamEvent, render_tideline_stream,
    tideline_stream_hitboxes,
};

fn events() -> Vec<TidelineStreamEvent> {
    use TidelineStreamEvent::*;
    vec![
        UserTurn {
            text: "ship the tideline work stage".to_string(),
        },
        PodFormation {
            edges: vec![
                (false, "whale-1 · restyle the footer band".to_string()),
                (false, "whale-2 · golden buffers".to_string()),
                (true, "whale-3 · preview render".to_string()),
            ],
        },
        Receipt {
            state: TidelineReceiptState::Working,
            label: "whale-1 editing crates/tui".to_string(),
            at: "14:41:02".to_string(),
            count: Some(12),
        },
        Receipt {
            state: TidelineReceiptState::Done,
            label: "whale-2 wrote 4 goldens".to_string(),
            at: "14:39:02".to_string(),
            count: Some(34),
        },
        Receipt {
            state: TidelineReceiptState::Failed,
            label: "whale-3 preview render".to_string(),
            at: "14:20:55".to_string(),
            count: Some(3),
        },
        AssistantTurn {
            text: "two whales surfaced; the third failed — retrying".to_string(),
        },
        Conclusion {
            text: "done: footer merged, 4 goldens blessed".to_string(),
        },
    ]
}

fn groups() -> Vec<super::TidelineRailGroup> {
    tideline_rail_groups(
        "release 0.9.12",
        "3/4 whales",
        "launch fleet",
        &["▸ footer band", "  goldens ×4"],
        61,
    )
}

/// Render the stage directly (the fixture borrows cannot outlive the fn).
fn draw_stage(width: u16, height: u16) -> String {
    render_golden_text(width, height, |buf| {
        let groups = groups();
        let events = events();
        let stage = TidelineWorkStage {
            rail: TidelineRail::new(&UI_THEME, &groups).focused(true),
            stream: TidelineStream::new(&UI_THEME, &events).selected(Some(3)),
        };
        render_tideline_work_stage(Rect::new(0, 0, width, height), buf, &stage);
    })
}

#[test]
fn work_stage_matches_goldens_at_blocker_sizes() {
    for (w, h) in BLOCKER_SIZES {
        assert_matches_golden(&format!("work_{w}x{h}"), &draw_stage(w, h));
    }
}

#[test]
fn work_stage_rail_width_ladder_hides_below_hundred() {
    assert_eq!(tideline_rail_width(160), 22);
    assert_eq!(tideline_rail_width(120), 22);
    assert_eq!(tideline_rail_width(100), 16);
    assert_eq!(tideline_rail_width(80), 0, "rail sheds at 80 (§5b ⑦)");
    // The 80-wide stage has no rail: the stream starts at column 0.
    let text = draw_stage(80, 24);
    assert!(
        !text.contains("RUNS"),
        "rail hidden at 80: first line is the stream: {text}"
    );
    assert!(text.contains("ship the tideline work stage"), "{text}");
}

#[test]
fn rail_shows_five_groups_help_and_collapse() {
    let groups = groups();
    let rail = TidelineRail::new(&UI_THEME, &groups).focused(true);
    let text = render_golden_text(22, 30, |buf| {
        render_tideline_rail(Rect::new(0, 0, 22, 30), buf, &rail);
    });
    for label in ["RUNS", "WHALES", "FLEET", "WORK", "CONTEXT"] {
        assert!(text.contains(label), "missing {label}: {text}");
    }
    assert!(text.contains("3/4 whales"), "{text}");
    assert!(text.contains("▰▰▰▱▱ 61%"), "context meter: {text}");
    assert!(text.contains("? help"), "{text}");
    assert!(text.contains("« collapse"), "{text}");
}

#[test]
fn rail_collapsed_leaves_only_the_expander() {
    let groups = groups();
    let rail = TidelineRail::new(&UI_THEME, &groups).collapsed(true);
    let text = render_golden_text(22, 30, |buf| {
        render_tideline_rail(Rect::new(0, 0, 22, 30), buf, &rail);
    });
    assert!(text.contains('»'), "expander: {text}");
    assert!(!text.contains("RUNS"), "groups hidden: {text}");
}

#[test]
fn stream_renders_turns_tree_receipts_conclusion_and_legend() {
    let events = events();
    let stream = TidelineStream::new(&UI_THEME, &events).selected(Some(3));
    let text = render_golden_text(60, 24, |buf| {
        render_tideline_stream(Rect::new(0, 0, 60, 24), buf, &stream);
    });
    assert!(text.contains("▎ ship the tideline work stage"), "{text}");
    assert!(text.contains("├── whale-1"), "tree branch: {text}");
    assert!(text.contains("└── whale-3"), "tree last edge: {text}");
    assert!(text.contains("×12"), "receipt count: {text}");
    assert!(text.contains("working whale-1 editing"), "{text}");
    assert!(text.contains("failed whale-3"), "{text}");
    // Selection never erases a state mark: at the stream's left edge the
    // ✓ stays and the focus rides on bold (the work-stage golden shows the
    // ▸ marker in its own column when the rail provides one).
    assert!(
        text.contains("✓ done whale-2"),
        "state mark survives selection: {text}"
    );
    assert!(text.trim_end().contains("done"), "conclusion: {text}");
    // The legend teaches every mark in place.
    assert!(text.contains("● working"), "{text}");
    assert!(text.contains("! caution"), "{text}");
    assert!(text.contains("✗ failed"), "{text}");
}

#[test]
fn stream_ascii_safe_projects_marks_and_tree() {
    let events = events();
    let stream = TidelineStream::new(&UI_THEME, &events).ascii_safe(true);
    let text = render_golden_text(60, 24, |buf| {
        render_tideline_stream(Rect::new(0, 0, 60, 24), buf, &stream);
    });
    // The charter maps both ├── and └── to `+` (spec §2 table), so the
    // last edge reads by position, not glyph shape.
    assert!(text.contains("+-- whale-1"), "tree ascii: {text}");
    assert!(text.contains("+-- whale-3"), "last-edge ascii: {text}");
    assert!(text.contains(". working"), "mark ascii: {text}");
    for ch in text.chars() {
        if ch != '\n' {
            assert_eq!(ch.width(), Some(1), "ascii-safe single-width: {ch:?}");
        }
    }
}

#[test]
fn work_stage_hitboxes_match_painted_rows() {
    let (w, h) = (120, 32);
    let area = Rect::new(0, 0, w, h);
    let groups = groups();
    let events = events();
    let rail = TidelineRail::new(&UI_THEME, &groups).focused(true);
    let stream = TidelineStream::new(&UI_THEME, &events).selected(Some(3));
    let stage = TidelineWorkStage {
        rail: TidelineRail::new(&UI_THEME, &groups).focused(true),
        stream: TidelineStream::new(&UI_THEME, &events).selected(Some(3)),
    };
    let mut buf = Buffer::empty(area);
    render_tideline_work_stage(area, &mut buf, &stage);

    let rail_rect = Rect::new(0, 0, 22, h);
    let rail_boxes = tideline_rail_hitboxes(rail_rect, &rail);
    assert_eq!(rail_boxes.len(), 5, "one rect per group");
    for rect in &rail_boxes {
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(!cells.trim().is_empty(), "rail rect covers empty cells");
    }

    let stream_area = Rect::new(22, 0, w - 22, h);
    let stream_boxes = tideline_stream_hitboxes(stream_area, &stream);
    assert_eq!(stream_boxes.len(), events.len(), "one rect per event");
    // The fleet tree's rect spans its three edge rows.
    assert_eq!(stream_boxes[1].height, 3);
    for rect in &stream_boxes {
        let cells: String = (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y)].symbol().to_string())
            .collect();
        assert!(!cells.trim().is_empty(), "stream rect covers empty cells");
    }
}

#[test]
fn work_stage_degenerate_sizes_do_not_panic() {
    for (w, h) in [(0u16, 0), (8, 1), (10, 1), (300, 60)] {
        let _ = draw_stage(w, h);
        let groups = groups();
        let collapsed = TidelineRail::new(&UI_THEME, &groups).collapsed(true);
        let _ = render_golden_text(w, h, |buf| {
            render_tideline_rail(Rect::new(0, 0, w.max(1), h.max(1)), buf, &collapsed);
        });
    }
}
