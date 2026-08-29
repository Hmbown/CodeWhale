//! Line-list rail panels, ported from the legacy classic-shell sidebar
//! during the 0.9.4 rail unification (spec step 2). Only **Context** still
//! renders this way: its lines are session facts, not work rows, so there
//! is nothing to click. Tasks, Agents, and Pinned all render through the
//! row/hitbox machinery in `render.rs` — a work row is a selectable,
//! clickable object in every panel, and a panel is just a subset of the one
//! work list.
//!
//! On Top placement the strip auto-fits its content the way Tasks always
//! did (and the way GrokBuild's tasks pane does): a two-agent fan-out is
//! two rows, not a fixed four-row band with a chrome title. Auto-fit
//! governs HEIGHT only, never membership — a settled to-do or finished
//! sub-agent still occupies a row (quiet completion, not eviction). The
//! only Top title is an active **goal** (not panel names like "Pinned").
//!
//! The line builders themselves still live in `tui::sidebar` (they are
//! `pub(crate)` there) while the sidebar module is wound down. The Agents
//! and Pinned arms below are retained for that wind-down but are no longer
//! reachable from the rail, which routes those panels through rows.

use ratatui::text::Line;

use crate::tui::app::App;
use crate::tui::sidebar::{self, SidebarSubagentSummary, WorkPanelOpts};
use crate::tui::subagent_routing::active_fanout_counts;

use super::model::RailPanel;

/// Cap used when measuring natural content height so a pathological
/// checklist cannot allocate unbounded lines during layout. The strip's
/// real cap (`top_cap`) still clamps the visible window.
const NATURAL_HEIGHT_PROBE: usize = 64;

/// Display lines for a non-Tasks rail panel, or `None` for Tasks (which the
/// caller renders through the row machinery instead).
///
/// `omit_goal_objective` is set on Top when the goal is already the strip
/// title, so Pinned does not repeat `Goal: …` in the body.
pub(crate) fn panel_lines(
    app: &mut App,
    panel: RailPanel,
    content_width: usize,
    max_rows: usize,
    omit_goal_objective: bool,
) -> Option<Vec<Line<'static>>> {
    let content_width = content_width.max(1);
    let max_rows = max_rows.max(1);
    match panel {
        RailPanel::Tasks => None,
        RailPanel::Agents => Some(agents_panel_lines(app, content_width, max_rows)),
        RailPanel::Context => Some(sidebar::context_panel_lines(app, content_width)),
        RailPanel::Pinned => Some(pinned_panel_lines(
            app,
            content_width,
            max_rows,
            omit_goal_objective,
        )),
    }
}

/// Whether a non-Tasks panel has anything worth spending a top-strip row on.
/// Empty projections collapse to zero the way Tasks does — an empty panel is
/// not a panel. Context always has session facts, so it always has content.
pub(crate) fn panel_has_useful_content(app: &mut App, panel: RailPanel) -> bool {
    match panel {
        RailPanel::Tasks => true,
        RailPanel::Pinned => sidebar::sidebar_work_summary(app).has_useful_content(),
        RailPanel::Agents => agents_have_useful_content(app),
        RailPanel::Context => true,
    }
}

/// Natural content row count for height auto-fit. Does not include the
/// divider row or the optional Top goal title that `height()` adds.
pub(crate) fn panel_content_row_count(
    app: &mut App,
    panel: RailPanel,
    content_width: usize,
    omit_goal_objective: bool,
) -> usize {
    panel_lines(
        app,
        panel,
        content_width,
        NATURAL_HEIGHT_PROBE,
        omit_goal_objective,
    )
    .map(|lines| lines.len())
    .unwrap_or(0)
}

fn agents_have_useful_content(app: &App) -> bool {
    if !app.subagent_cache.is_empty() {
        return true;
    }
    if !app.agent_progress.is_empty() {
        return true;
    }
    if active_fanout_counts(app).is_some_and(|(_, total)| total > 0) {
        return true;
    }
    sidebar::foreground_rlm_running(app)
}

/// Agents panel: cached sub-agents plus progress-only and fanout signals.
/// The summary projection is lifted from the legacy `render_sidebar_subagents`
/// so the panel keeps its exact content in the rail.
fn agents_panel_lines(app: &App, content_width: usize, max_rows: usize) -> Vec<Line<'static>> {
    let cached_ids: std::collections::HashSet<&str> = app
        .subagent_cache
        .iter()
        .map(|agent| agent.agent_id.as_str())
        .collect();
    let progress_only_count = app
        .agent_progress
        .keys()
        .filter(|id| !cached_ids.contains(id.as_str()))
        .count();
    let cached_running = app
        .subagent_cache
        .iter()
        .filter(|agent| sidebar::cached_agent_activity_is_live(app, agent))
        .count();
    let role_counts: std::collections::BTreeMap<String, usize> =
        app.subagent_cache
            .iter()
            .fold(std::collections::BTreeMap::new(), |mut acc, agent| {
                *acc.entry(agent.agent_type.as_str().to_string())
                    .or_insert(0) += 1;
                acc
            });
    let (fanout_running, fanout_total) = active_fanout_counts(app)
        .map(|(running, total)| (running, Some(total)))
        .unwrap_or((0, None));
    let summary = SidebarSubagentSummary {
        cached_total: app.subagent_cache.len(),
        cached_running,
        progress_only_count,
        fanout_total,
        fanout_running,
        foreground_rlm_running: sidebar::foreground_rlm_running(app),
        role_counts,
    };
    let rows = sidebar::sidebar_agent_rows(app);
    sidebar::subagent_panel_lines(
        &summary,
        &rows,
        app.ui_locale,
        content_width,
        max_rows,
        &app.ui_theme,
    )
}

/// Pinned panel: the durable work summary (goal + checklist) the legacy
/// sidebar showed in Pinned focus.
fn pinned_panel_lines(
    app: &mut App,
    content_width: usize,
    max_rows: usize,
    omit_goal_objective: bool,
) -> Vec<Line<'static>> {
    let summary = sidebar::sidebar_work_summary(app);
    sidebar::work_panel_lines_with_opts(
        &summary,
        content_width,
        max_rows,
        app.ui_theme.mode,
        &app.ui_theme,
        WorkPanelOpts {
            omit_goal_objective,
        },
    )
}

// ---------------------------------------------------------------------------
// Tideline pod ledger (spec §2 ledger resolution, §5a "Pod ledger", §5b
// ledger columns): the whale table with fixed columns and per-column
// truncation — never wrap. Replaces the workflow-panel duplicate as the one
// whale surface. Translation scaffolding in the topbar mold: pure,
// deterministic, injected rows (the caller projects `subagent_cache` +
// worker runtime states); wired into the work stage at the landing slice
// (#5698 gate). #5699's shell semantics are untouched — this only adds the
// Tideline rendering.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
};
use unicode_width::UnicodeWidthStr;

use crate::palette::{ChromeInk, UiTheme, chrome_style};

/// WHALE column width — names are short by contract and never truncate.
const WHALE_CELLS: usize = 10;
/// STATE column width — glyph + word, e.g. `● running`.
const STATE_CELLS: usize = 12;
/// Time-column widths (ELAPSED, RECEIPTS, LAST UPDATE).
const TIME_CELLS: usize = 8;

/// Whale runtime state for the ledger row (§5d marks table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub enum TidelineWhaleState {
    /// ● working — Active ink.
    Working,
    /// ✓ done — Outcome ink.
    Done,
    /// ! caution — Attention ink (color never invents state; the word says it).
    Caution,
    /// ✗ failed — Failure ink (red stays failure-only).
    Failed,
    /// ○ idle — ready but not active.
    Idle,
}

impl TidelineWhaleState {
    /// Glyph + word within STATE_CELLS.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => "● working",
            Self::Done => "✓ done",
            Self::Caution => "! caution",
            Self::Failed => "✗ failed",
            Self::Idle => "○ idle",
        }
    }

    #[must_use]
    pub fn ink(self) -> ChromeInk {
        match self {
            Self::Working => ChromeInk::Active,
            Self::Done => ChromeInk::Outcome,
            Self::Caution => ChromeInk::Attention,
            Self::Failed => ChromeInk::Failure,
            Self::Idle => ChromeInk::MetadataDim,
        }
    }
}

/// One ledger row. Every field already formatted by the caller (elapsed,
/// receipts count, HH:MM:SS clock) so renders stay deterministic.
#[derive(Debug, Clone)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineLedgerRow {
    /// Whale name, ≤10 cells by contract — never truncated.
    pub whale: String,
    /// Assignment objective; truncated with `…`, never wrapped.
    pub assignment: String,
    pub state: TidelineWhaleState,
    /// Elapsed label, e.g. `1m 15s`.
    pub elapsed: String,
    /// Receipt count label, e.g. `12`.
    pub receipts: String,
    /// Last-update clock `HH:MM:SS`.
    pub last_update: String,
}

/// The visible column set for a main-area width (spec §5b): time columns
/// shed LAST UPDATE → RECEIPTS → ELAPSED before ASSIGNMENT loses cells; at
/// 80 columns the ledger is WHALE │ ASSIGNMENT │ STATE.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelineLedgerColumns {
    pub elapsed: bool,
    pub receipts: bool,
    pub last_update: bool,
}

#[allow(dead_code)] // translation scaffolding: builder/convenience methods feed tests + the landing slice
impl TidelineLedgerColumns {
    #[must_use]
    pub fn for_width(width: u16) -> Self {
        if width >= 130 {
            Self {
                elapsed: true,
                receipts: true,
                last_update: true,
            }
        } else if width >= 110 {
            Self {
                elapsed: true,
                receipts: true,
                last_update: false,
            }
        } else {
            Self {
                elapsed: false,
                receipts: false,
                last_update: false,
            }
        }
    }

    /// Column headers in display order.
    #[must_use]
    pub fn headers(self) -> Vec<&'static str> {
        let mut out = vec!["WHALE", "ASSIGNMENT", "STATE"];
        if self.elapsed {
            out.push("ELAPSED");
        }
        if self.receipts {
            out.push("RECEIPTS");
        }
        if self.last_update {
            out.push("LAST UPDATE");
        }
        out
    }
}

/// What the caller owes the ledger render.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub struct TidelinePodLedger<'a> {
    pub theme: &'a UiTheme,
    pub rows: &'a [TidelineLedgerRow],
    /// Selected row — `▶` marker; Enter/click inspects beside the evidence.
    pub selected: usize,
    pub ascii_safe: bool,
}

#[allow(dead_code)] // translation scaffolding: builder methods feed tests + the landing slice
impl<'a> TidelinePodLedger<'a> {
    #[allow(dead_code)] // translation scaffolding: wired by the landing slice
    #[must_use]
    pub fn new(theme: &'a UiTheme, rows: &'a [TidelineLedgerRow]) -> Self {
        Self {
            theme,
            rows,
            selected: 0,
            ascii_safe: false,
        }
    }

    #[must_use]
    pub fn selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    #[must_use]
    pub fn ascii_safe(mut self, ascii_safe: bool) -> Self {
        self.ascii_safe = ascii_safe;
        self
    }

    fn sym(&self, glyph: &str) -> String {
        if !self.ascii_safe {
            return glyph.to_string();
        }
        if let Some(fb) = crate::tui::glyphs::ascii_fallback(glyph) {
            return fb.to_string();
        }
        glyph
            .chars()
            .map(|c| {
                crate::tui::glyphs::ascii_fallback(&c.to_string())
                    .map(str::to_string)
                    .unwrap_or_else(|| c.to_string())
            })
            .collect()
    }
}

fn lchrome(theme: &UiTheme, ink: ChromeInk) -> Style {
    chrome_style(theme, ink)
}

fn lput(buf: &mut Buffer, x: u16, y: u16, text: &str, style: Style) {
    buf.set_stringn(x, y, text, text.width(), style);
}

fn ltruncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let ellipsis = "…";
    let mut out = String::new();
    let mut used = 0;
    let budget = width.saturating_sub(1);
    for ch in text.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push_str(ellipsis);
    out
}

/// Paint the pod ledger: `POD LEDGER` title, column header row, one-line
/// rows (truncate, never wrap) with the selected-row `▶` marker.
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn render_tideline_ledger(area: Rect, buf: &mut Buffer, ledger: &TidelinePodLedger<'_>) {
    if area.width < 30 || area.height < 2 {
        return;
    }
    let theme = ledger.theme;
    let columns = TidelineLedgerColumns::for_width(area.width);

    lput(
        buf,
        area.x,
        area.y,
        "POD LEDGER",
        lchrome(theme, ChromeInk::Metadata).add_modifier(Modifier::BOLD),
    );

    // Column x positions: marker col (2) then WHALE │ ASSIGNMENT │ STATE [│ time…].
    let sep = ledger.sym("│");
    let sep_w = sep.width() as u16;
    let mut x = area.x + 2;
    let whale_x = x;
    x += WHALE_CELLS as u16 + sep_w;
    let assignment_x = x;
    let assignment_w = {
        let mut w = area
            .width
            .saturating_sub(2 + (WHALE_CELLS as u16) + sep_w + (STATE_CELLS as u16) + 2 * sep_w);
        if columns.elapsed {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        if columns.receipts {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        if columns.last_update {
            w = w.saturating_sub(TIME_CELLS as u16 + sep_w);
        }
        w
    };
    x += assignment_w + sep_w;
    let state_x = x;

    // Header row.
    let header_y = area.y + 1;
    lput(
        buf,
        whale_x,
        header_y,
        "WHALE",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    paint_sep(buf, ledger, theme, whale_x + WHALE_CELLS as u16, header_y);
    lput(
        buf,
        assignment_x,
        header_y,
        "ASSIGNMENT",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    paint_sep(buf, ledger, theme, assignment_x + assignment_w, header_y);
    lput(
        buf,
        state_x,
        header_y,
        "STATE",
        lchrome(theme, ChromeInk::MetadataDim),
    );
    let mut hx = state_x + STATE_CELLS as u16;
    if columns.elapsed {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            "ELAPSED",
            lchrome(theme, ChromeInk::MetadataDim),
        );
        hx += TIME_CELLS as u16;
    }
    if columns.receipts {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            "RECEIPTS",
            lchrome(theme, ChromeInk::MetadataDim),
        );
        hx += TIME_CELLS as u16;
    }
    if columns.last_update {
        paint_sep(buf, ledger, theme, hx, header_y);
        hx += sep_w;
        lput(
            buf,
            hx,
            header_y,
            &ltruncate("LAST UPDATE", TIME_CELLS),
            lchrome(theme, ChromeInk::MetadataDim),
        );
    }

    // Rows: one line each, selected marker `▶`.
    for (index, row) in ledger.rows.iter().enumerate() {
        let y = area.y + 2 + index as u16;
        if y >= area.y + area.height {
            break;
        }
        let selected = ledger.selected == index;
        if selected {
            lput(
                buf,
                area.x,
                y,
                &ledger.sym("▶"),
                lchrome(theme, ChromeInk::Identity),
            );
        }
        lput(
            buf,
            whale_x,
            y,
            &ledger.sym(&row.whale),
            lchrome(theme, ChromeInk::Identity),
        );
        paint_sep(buf, ledger, theme, whale_x + WHALE_CELLS as u16, y);
        lput(
            buf,
            assignment_x,
            y,
            &ltruncate(&ledger.sym(&row.assignment), assignment_w as usize),
            lchrome(theme, ChromeInk::MetadataValue),
        );
        paint_sep(buf, ledger, theme, assignment_x + assignment_w, y);
        lput(
            buf,
            state_x,
            y,
            &ledger.sym(row.state.label()),
            lchrome(theme, row.state.ink()),
        );
        let mut tx = state_x + STATE_CELLS as u16;
        if columns.elapsed {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.elapsed),
                lchrome(theme, ChromeInk::Metadata),
            );
            tx += TIME_CELLS as u16;
        }
        if columns.receipts {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.receipts),
                lchrome(theme, ChromeInk::Metadata),
            );
            tx += TIME_CELLS as u16;
        }
        if columns.last_update {
            paint_sep(buf, ledger, theme, tx, y);
            tx += sep_w;
            lput(
                buf,
                tx,
                y,
                &ledger.sym(&row.last_update),
                lchrome(theme, ChromeInk::MetadataHint),
            );
        }
    }
}

fn paint_sep(buf: &mut Buffer, ledger: &TidelinePodLedger<'_>, theme: &UiTheme, x: u16, y: u16) {
    let sep = ledger.sym("│");
    lput(buf, x, y, &sep, lchrome(theme, ChromeInk::MetadataDim));
}

/// Row hitboxes → inspector (spec §6): one rect per visible row.
#[must_use]
#[allow(dead_code)] // translation scaffolding: wired by the landing slice
pub fn tideline_ledger_hitboxes(area: Rect, ledger: &TidelinePodLedger<'_>) -> Vec<Rect> {
    let mut out = Vec::new();
    if area.width < 30 || area.height < 2 {
        return out;
    }
    for index in 0..ledger.rows.len() {
        let y = area.y + 2 + index as u16;
        if y >= area.y + area.height {
            break;
        }
        out.push(Rect {
            x: area.x,
            y,
            width: area.width,
            height: 1,
        });
    }
    out
}

#[cfg(test)]
mod tideline_tests;
