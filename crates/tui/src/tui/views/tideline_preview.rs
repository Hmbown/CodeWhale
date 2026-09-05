//! Tideline settings live preview (spec §5a "Live preview"): a **real
//! projection of the real renderers** — the receipt stream, fleet ledger,
//! composer chrome, and merged footer all render through their actual
//! functions with a candidate theme injected. No second store, no mock
//! markup: what the preview paints is what ships. Esc restoring the prior
//! theme cannot flicker because the projection mutates nothing.
//!
//! Translation scaffolding in the topbar mold; wired into the settings
//! stage at the landing slice (#5698 gate).

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::Style,
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme, chrome_style};
use crate::tui::composer_chrome::TidelineComposer;
use crate::tui::history::tideline_exports::{
    TidelineReceiptState, TidelineStream, TidelineStreamEvent,
};
use crate::tui::phase_strip::TidelineFooter;
use crate::tui::work_surface::panels::{TidelineLedgerRow, TidelinePodLedger, TidelineWhaleState};
use crate::tui::work_surface::tideline::render_tideline_rail;

/// Fixture transcript the preview replays — small, deterministic, and
/// exercising every stream shape (turn, tree, receipts, conclusion).
#[must_use]
pub fn tideline_preview_events() -> Vec<TidelineStreamEvent> {
    use TidelineStreamEvent::*;
    vec![
        UserTurn {
            text: "restyle the work stage".to_string(),
        },
        PodFormation {
            edges: vec![
                (false, "whale-1 · footer band".to_string()),
                (true, "whale-2 · goldens".to_string()),
            ],
        },
        Receipt {
            state: TidelineReceiptState::Working,
            label: "whale-1 editing".to_string(),
            at: "14:41:02".to_string(),
            count: Some(12),
        },
        Receipt {
            state: TidelineReceiptState::Done,
            label: "whale-2 surfaced ✓".to_string(),
            at: "14:39:02".to_string(),
            count: Some(34),
        },
        Conclusion {
            text: "done: stage restyled".to_string(),
        },
    ]
}

/// Fixture ledger rows for the preview pane.
#[must_use]
pub fn tideline_preview_ledger_rows() -> Vec<TidelineLedgerRow> {
    vec![
        TidelineLedgerRow {
            whale: "whale-1".to_string(),
            assignment: "restyle the footer band".to_string(),
            state: TidelineWhaleState::Working,
            elapsed: "1m 15s".to_string(),
            receipts: "12".to_string(),
            last_update: "14:42:18".to_string(),
        },
        TidelineLedgerRow {
            whale: "whale-2".to_string(),
            assignment: "golden buffers".to_string(),
            state: TidelineWhaleState::Done,
            elapsed: "2m 03s".to_string(),
            receipts: "34".to_string(),
            last_update: "14:39:02".to_string(),
        },
    ]
}

/// The preview projection. `candidate` is the theme under the cursor.
pub struct TidelineSettingsPreview<'a> {
    pub active_theme: &'a UiTheme,
    pub candidate: &'a UiTheme,
    pub candidate_name: &'a str,
    pub ascii_safe: bool,
}

fn pput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

/// Paint the live preview: header naming the candidate theme, then the
/// projected work surface — rail, receipt stream, mini ledger, composer,
/// footer — all through their real render functions with `candidate`.
pub fn render_tideline_settings_preview(
    area: Rect,
    buf: &mut Buffer,
    preview: &TidelineSettingsPreview<'_>,
) {
    if area.width < 20 || area.height < 8 {
        return;
    }
    let candidate = preview.candidate;
    pput(
        buf,
        area.x,
        area.y,
        &format!("PREVIEW · {}", preview.candidate_name),
        chrome_style(candidate, ChromeInk::Metadata).add_modifier(ratatui::style::Modifier::BOLD),
    );

    let body = Rect {
        y: area.y + 1,
        height: area.height - 1,
        ..area
    };
    let [stream_area, ledger_area, composer_area, footer_area] = Layout::vertical([
        Constraint::Min(5),
        Constraint::Length(2 + 2),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(body);

    // Stream row: the rail appears only when the preview pane itself is
    // wide enough for the ladder (≥100) — same rule as the real stage.
    let rail_w = crate::tui::work_surface::tideline::tideline_rail_width(area.width);
    let (rail_area, stream_only) = if rail_w > 0 {
        let [rail_area, rest] =
            Layout::horizontal([Constraint::Length(rail_w), Constraint::Min(1)]).areas(stream_area);
        (Some(rail_area), rest)
    } else {
        (None, stream_area)
    };
    if let Some(rail_area) = rail_area {
        let groups = crate::tui::work_surface::tideline::tideline_rail_groups(
            "release 0.9.12",
            "2/4 whales",
            "launch fleet",
            &["▸ footer band"],
            61,
        );
        let rail = crate::tui::work_surface::tideline::TidelineRail::new(candidate, &groups);
        render_tideline_rail(rail_area, buf, &rail);
    }
    let events = tideline_preview_events();
    let stream = TidelineStream::new(candidate, &events);
    crate::tui::history::render_tideline_stream(stream_only, buf, &stream);

    let rows = tideline_preview_ledger_rows();
    let ledger = TidelinePodLedger::new(candidate, &rows).selected(0);
    crate::tui::work_surface::panels::render_tideline_ledger(ledger_area, buf, &ledger);

    let draft = "the preview is the product";
    let composer = TidelineComposer::new(candidate, draft).focused(true);
    crate::tui::composer_chrome::render_tideline_composer(composer_area, buf, &composer);

    let counts = [("2 agents".to_string(), ChromeInk::Active)];
    let footer = TidelineFooter::new(candidate, ("ask", ChromeInk::PermissionAsk))
        .permission_key(Some("Shift+Tab"))
        .mode_chip(Some(("work", ChromeInk::PolicyAct)))
        .mode_key(Some("Tab"))
        .counts(&counts)
        .hint(Some(("Esc to interrupt", ChromeInk::MetadataHint)))
        .context_percent(61);
    crate::tui::phase_strip::render_tideline_footer(footer_area, buf, &footer);
}
