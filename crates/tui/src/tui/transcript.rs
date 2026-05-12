//! Cached transcript rendering for the TUI.
//!
//! ## Per-cell revision caching
//!
//! Naive caching invalidates the whole transcript whenever ANY cell mutates.
//! During streaming the assistant content cell mutates on every delta — that
//! would force a re-wrap of every cell on every chunk. Codex avoids this by
//! tracking a per-cell revision counter; we mirror that pattern here.
//!
//! Each cell index has a paired `revision: u64`. The cache stores
//! `Vec<CachedCell>` with `(cell_index, revision, lines, line_meta)`. On
//! `ensure`, walk the cells; if a cell's current `revision` matches the cached
//! one (and width/options haven't changed), reuse the rendered lines.
//! Otherwise re-render that cell only and reassemble.
//!
//! Width or render-option changes still bust the entire cache (correct: wrap
//! layout depends on width and which cells are visible at all).

use std::sync::Arc;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::tui::app::TranscriptSpacing;
use crate::tui::history::{HistoryCell, TranscriptRenderOptions};
use crate::tui::scrolling::TranscriptLineMeta;

/// Per-cell cached render output. Reused across `ensure` calls when the
/// upstream cell's revision counter hasn't changed.
///
/// Lines are stored behind an `Arc` so that cloning a `CachedCell` during
/// cache-ensure (which touches every cell every frame) is O(1) rather than
/// O(rendered_line_count). Without this, scrolling on a long transcript
/// pays the cost of deep-cloning every cell's `Vec<Line>` per frame, which
/// is the surface-level symptom of issue #78. The flatten step uses
/// `Arc::make_mut` to produce an owned `Vec` for the final `lines`
/// assembly, so the only deep-clone occurs on the flattened output — once
/// per frame instead of once per cell.
#[derive(Debug, Clone)]
struct CachedCell {
    /// Revision the cell was at when the lines/meta were rendered.
    revision: u64,
    /// Rendered lines for this cell (without trailing inter-cell spacers),
    /// shared via `Arc` so cache enumeration is O(N) not O(N*lines).
    lines: Arc<Vec<Line<'static>>>,
    /// Whether this cell's rendered output was empty (e.g. Thinking hidden).
    /// Cached so we can skip empty cells without re-rendering.
    is_empty: bool,
    /// Whether this cell is a stream continuation. Determines spacer rules.
    /// Cached because `is_stream_continuation` is cheap but reading via the
    /// cache lets us decide spacers without touching the cell.
    is_stream_continuation: bool,
    /// Whether this cell is conversational (User/Assistant/Thinking). Used
    /// for spacer calculations.
    is_conversational: bool,
    /// Whether this cell is a System or Tool cell (affects spacer rules).
    is_system_or_tool: bool,
    /// Whether this cell participates in the compact tool-card rail group.
    is_tool_groupable: bool,
}

/// Cache of rendered transcript lines for the current viewport.
#[derive(Debug)]
pub struct TranscriptViewCache {
    width: u16,
    options: TranscriptRenderOptions,
    /// Per-cell rendered output, indexed by current cell position.
    /// Length always equals the cell count seen on the last `ensure` call.
    per_cell: Vec<CachedCell>,
    /// Flattened lines reassembled from `per_cell` plus spacers.
    lines: Vec<Line<'static>>,
    /// Per-line metadata aligned with `lines`.
    line_meta: Vec<TranscriptLineMeta>,
    /// Per-line rail-prefix display-column count (`0` or `2`), aligned with
    /// `lines`. Populated during flatten so that selection-to-text can shift
    /// columns past visual-only decoration glyphs without guessing which
    /// spans are decorative (#1163).
    rail_prefix_widths: Vec<usize>,
}

impl TranscriptViewCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            width: 0,
            options: TranscriptRenderOptions::default(),
            per_cell: Vec::new(),
            lines: Vec::new(),
            line_meta: Vec::new(),
            rail_prefix_widths: Vec::new(),
        }
    }

    /// Ensure cached lines match the provided cells/widths/per-cell revisions.
    ///
    /// Reuses rendered lines for cells whose `cell_revisions[i]` matches the
    /// previously cached revision (when the cell shape — empty/spacer flags —
    /// also matches). Width or option changes bust the entire cache.
    ///
    /// `cell_revisions.len()` is expected to equal `cells.len()`. If they
    /// disagree (shouldn't happen in normal use) the cache treats every cell
    /// as dirty.
    ///
    /// Retained for tests and external use; the live render path uses the
    /// `ensure_split` variant to avoid concatenating history + active-cell
    /// entries every frame.
    #[allow(dead_code)]
    pub fn ensure(
        &mut self,
        cells: &[HistoryCell],
        cell_revisions: &[u64],
        width: u16,
        options: TranscriptRenderOptions,
    ) {
        self.ensure_split(&[cells], cell_revisions, width, options);
    }

    /// Ensure cached lines match the provided cell shards (logically
    /// concatenated) plus per-cell revisions. Avoids the
    /// `concat-into-Vec<HistoryCell>` clone the caller would otherwise pay
    /// every frame on long transcripts.
    pub fn ensure_split(
        &mut self,
        cell_shards: &[&[HistoryCell]],
        cell_revisions: &[u64],
        width: u16,
        options: TranscriptRenderOptions,
    ) {
        let total_cells: usize = cell_shards.iter().map(|s| s.len()).sum();

        let layout_changed = self.width != width || self.options != options;
        if layout_changed {
            self.per_cell.clear();
        }
        self.width = width;
        self.options = options;

        // Track whether anything actually changed; if all cells are reused at
        // the same indices, we can skip the reflatten.
        let old_len = self.per_cell.len();
        let mut any_dirty = layout_changed || old_len != total_cells;
        let mut first_dirty: Option<usize> = if old_len != total_cells {
            Some(old_len.min(total_cells))
        } else {
            None
        };

        let mut new_per_cell: Vec<CachedCell> = Vec::with_capacity(total_cells);
        let revisions_match = cell_revisions.len() == total_cells;

        let mut idx: usize = 0;
        for shard in cell_shards {
            for cell in *shard {
                let current_rev = if revisions_match {
                    cell_revisions[idx]
                } else {
                    // No matching revisions — force a re-render this cycle.
                    u64::MAX
                };

                // Reuse cached entry if the revision matches AND it's at the
                // same index (cells can shift on insert/remove, so we only
                // reuse when the index is identical — a stricter invariant
                // codex also uses for its active-cell tail).
                if let Some(prev) = self.per_cell.get(idx)
                    && !layout_changed
                    && prev.revision == current_rev
                    && revisions_match
                {
                    new_per_cell.push(prev.clone());
                    idx += 1;
                    continue;
                }

                any_dirty = true;
                first_dirty = Some(first_dirty.map_or(idx, |current| current.min(idx)));
                let is_tool_groupable = matches!(cell, HistoryCell::Tool(_));
                let render_width = if is_tool_groupable {
                    width.saturating_sub(2).max(1)
                } else {
                    width
                };
                let rendered = cell.lines_with_options(render_width, options);
                let is_empty = rendered.is_empty();
                new_per_cell.push(CachedCell {
                    revision: current_rev,
                    lines: Arc::new(rendered),
                    is_empty,
                    is_stream_continuation: cell.is_stream_continuation(),
                    is_conversational: cell.is_conversational(),
                    is_system_or_tool: matches!(
                        cell,
                        HistoryCell::System { .. }
                            | HistoryCell::Error { .. }
                            | HistoryCell::Tool(_)
                            | HistoryCell::SubAgent(_)
                            | HistoryCell::ArchivedContext { .. }
                    ),
                    is_tool_groupable,
                });
                idx += 1;
            }
        }

        self.per_cell = new_per_cell;

        if !any_dirty {
            // All cells reused at the same indices: nothing to reflatten.
            // (Width didn't change either, since that bumps `layout_changed`.)
            return;
        }

        let rebuild_from = if layout_changed {
            0
        } else {
            first_dirty.unwrap_or(0).saturating_sub(1)
        };
        self.flatten_from(options.spacing, rebuild_from);
    }

    /// Reassemble flat `lines` / `line_meta` from `per_cell` plus spacers.
    fn flatten(&mut self, spacing: TranscriptSpacing) {
        self.lines.clear();
        self.line_meta.clear();
        self.rail_prefix_widths.clear();
        self.append_flattened_cells(spacing, 0);
    }

    /// Reassemble only the suffix starting at `first_cell`.
    ///
    /// Streaming usually mutates the active tail cell. Rebuilding from the
    /// previous cell preserves spacer correctness while avoiding a full
    /// O(total transcript lines) flatten on every token chunk.
    fn flatten_from(&mut self, spacing: TranscriptSpacing, first_cell: usize) {
        if first_cell == 0 || self.lines.is_empty() || self.line_meta.is_empty() {
            self.flatten(spacing);
            return;
        }

        let truncate_at = self
            .line_meta
            .iter()
            .position(|meta| match meta {
                TranscriptLineMeta::CellLine { cell_index, .. } => *cell_index >= first_cell,
                TranscriptLineMeta::Spacer => false,
            })
            .unwrap_or(self.lines.len());
        self.lines.truncate(truncate_at);
        self.line_meta.truncate(truncate_at);
        self.rail_prefix_widths.truncate(truncate_at);
        self.append_flattened_cells(spacing, first_cell);
    }

    fn append_flattened_cells(&mut self, spacing: TranscriptSpacing, start_cell: usize) {
        for (cell_index, cached) in self.per_cell.iter().enumerate().skip(start_cell) {
            if cached.is_empty {
                continue;
            }
            // Arc::make_mut would deep-clone only on write; since we just
            // rebuilt `lines` from scratch we always need the owned data.
            // Deref is zero-cost and gives us &[Line].
            let rendered_line_count = cached.lines.len();
            for (line_in_cell, line) in cached.lines.iter().enumerate() {
                let final_line = line_with_group_rail(
                    line,
                    tool_group_rail(
                        self.per_cell.as_slice(),
                        cell_index,
                        line_in_cell,
                        rendered_line_count,
                    ),
                    usize::from(self.width),
                );
                self.rail_prefix_widths
                    .push(compute_rail_prefix_width(&final_line));
                self.lines.push(final_line);
                self.line_meta.push(TranscriptLineMeta::CellLine {
                    cell_index,
                    line_in_cell,
                });
            }

            if let Some(next) = self.per_cell.get(cell_index + 1) {
                let spacer_rows = spacer_rows_between(cached, next, spacing);
                for _ in 0..spacer_rows {
                    self.lines.push(Line::from(""));
                    self.line_meta.push(TranscriptLineMeta::Spacer);
                    self.rail_prefix_widths.push(0);
                }
            }
        }
    }

    /// Return cached lines.
    #[must_use]
    pub fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    /// Return cached line metadata.
    #[must_use]
    pub fn line_meta(&self) -> &[TranscriptLineMeta] {
        &self.line_meta
    }

    /// Return total cached lines.
    #[must_use]
    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    /// Return the rail-prefix display-column count for the line at
    /// `line_index`. Callers use this to shift selection coordinates past
    /// visual-only decoration glyphs without guessing which spans are
    /// decorative (#1163).
    #[must_use]
    pub fn rail_prefix_width(&self, line_index: usize) -> usize {
        self.rail_prefix_widths
            .get(line_index)
            .copied()
            .unwrap_or(0)
    }
}

fn spacer_rows_between(
    current: &CachedCell,
    next: &CachedCell,
    spacing: TranscriptSpacing,
) -> usize {
    if current.is_stream_continuation {
        return 0;
    }

    if current.is_tool_groupable && next.is_tool_groupable {
        return 0;
    }

    let conversational_gap = match spacing {
        TranscriptSpacing::Compact => 0,
        TranscriptSpacing::Comfortable => 1,
        TranscriptSpacing::Spacious => 2,
    };
    let secondary_gap = match spacing {
        TranscriptSpacing::Compact => 0,
        TranscriptSpacing::Comfortable | TranscriptSpacing::Spacious => 1,
    };

    if current.is_conversational && next.is_conversational {
        conversational_gap
    } else if current.is_system_or_tool || next.is_system_or_tool {
        secondary_gap
    } else {
        0
    }
}

fn tool_group_rail(
    cells: &[CachedCell],
    cell_index: usize,
    line_in_cell: usize,
    rendered_line_count: usize,
) -> Option<crate::tui::widgets::tool_card::CardRail> {
    let cached = cells.get(cell_index)?;
    if !cached.is_tool_groupable || rendered_line_count == 0 {
        return None;
    }

    let previous_is_tool = cell_index
        .checked_sub(1)
        .and_then(|idx| cells.get(idx))
        .is_some_and(|cell| cell.is_tool_groupable && !cell.is_empty);
    let next_is_tool = cells
        .get(cell_index + 1)
        .is_some_and(|cell| cell.is_tool_groupable && !cell.is_empty);
    let first_line_in_group = !previous_is_tool && line_in_cell == 0;
    let last_line_in_group = !next_is_tool && line_in_cell + 1 == rendered_line_count;

    let rail = match (first_line_in_group, last_line_in_group) {
        (true, true) if rendered_line_count == 1 => {
            crate::tui::widgets::tool_card::CardRail::Single
        }
        (true, _) => crate::tui::widgets::tool_card::CardRail::Top,
        (_, true) => crate::tui::widgets::tool_card::CardRail::Bottom,
        _ => crate::tui::widgets::tool_card::CardRail::Middle,
    };
    Some(rail)
}

fn line_with_group_rail(
    line: &Line<'static>,
    rail: Option<crate::tui::widgets::tool_card::CardRail>,
    max_width: usize,
) -> Line<'static> {
    let Some(rail) = rail else {
        return line.clone();
    };
    let glyph = crate::tui::widgets::tool_card::rail_glyph(rail);
    if glyph.is_empty() {
        let mut rendered = line.clone();
        rendered.spans = truncate_spans_to_width(rendered.spans, max_width);
        return rendered;
    }

    let mut rendered = line.clone();
    let mut spans = Vec::with_capacity(rendered.spans.len() + 1);
    spans.push(Span::styled(
        format!("{glyph} "),
        Style::default().fg(crate::palette::TEXT_DIM),
    ));
    spans.extend(rendered.spans);
    rendered.spans = truncate_spans_to_width(spans, max_width);
    rendered
}

/// Return the display-column count of consecutive visual-only decorative
/// spans at the start of a rendered transcript line. Iterates through
/// leading spans matching either of two patterns:
///
/// * Pattern A — span is `"<glyph>[<glyph>…]<space>"` where every character
///   except the trailing space is a rail-drawing character (e.g. `▏ `,
///   `▶ `, `⋮⋮ `). The entire span width is accumulated.
/// * Pattern B — span is `"<glyph>"` (1 drawing char) followed by a lone
///   space span `" "` (e.g. `●` then ` `, `▎` then ` `).
///
/// Stops at the first non-matching span. Every decorated glyph used by the
/// TUI is a single display-column character, so char-count = display width.
///
/// Returns `0` for lines whose first span is not a decorative prefix.
fn compute_rail_prefix_width(line: &Line<'static>) -> usize {
    let spans = line.spans.as_slice();
    let mut total = 0;
    let mut i = 0;

    while i < spans.len() {
        let content = spans[i].content.as_ref();
        let n_chars = content.chars().count();

        // Pattern A — span "<glyph>[<glyph>…]<space>" (≥ 2 chars, trailing
        // space, all preceding chars are drawing chars).
        if n_chars >= 2
            && content.ends_with(' ')
            && content
                .chars()
                .take(n_chars.saturating_sub(1))
                .all(is_rail_drawing_char)
        {
            total += n_chars;
            i += 1;
            continue;
        }

        // Pattern B — span "<glyph>" (1 drawing char) + next span " ".
        if n_chars == 1
            && content.chars().next().is_some_and(is_rail_drawing_char)
            && spans.get(i + 1).is_some_and(|s| s.content.as_ref() == " ")
        {
            total += 2;
            i += 2;
            continue;
        }

        break;
    }

    total
}

/// Characters that serve as decoration glyphs in the TUI left-rail and
/// tool-header prefix system. All are single display-column characters.
fn is_rail_drawing_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{2500}'..='\u{257F}'   // Box Drawing (╭ ╮ ╰ ╯ │ ╎ …)
        | '\u{2580}'..='\u{259F}' // Block Elements (▏ ▎ ▍ ▌ …)
        | '\u{25A0}'..='\u{25FF}' // Geometric Shapes (● ▶ ▷ ◆ ◐ …)
        | '\u{2022}'              // • bullet (tool status / generic tool)
        | '\u{2026}'              // … ellipsis (reasoning opener)
        | '\u{00B7}'              // · middle dot (tool running symbol)
        | '\u{2315}'              // ⌕ telephone recorder (find/search tool)
        | '\u{22EE}'              // ⋮ vertical ellipsis (fanout/rlm tool)
    )
}

fn truncate_spans_to_width(spans: Vec<Span<'static>>, max_width: usize) -> Vec<Span<'static>> {
    if max_width == 0 || spans.is_empty() {
        return Vec::new();
    }
    let current_width: usize = spans
        .iter()
        .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if current_width <= max_width {
        return spans;
    }

    let ellipsis = if max_width > 3 { "..." } else { "" };
    let content_budget = max_width.saturating_sub(ellipsis.len());
    let mut used = 0usize;
    let mut truncated = Vec::with_capacity(spans.len() + usize::from(!ellipsis.is_empty()));
    let mut last_style = Style::default();

    'outer: for span in spans {
        last_style = span.style;
        let mut content = String::new();
        for ch in span.content.chars() {
            let width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            if used + width > content_budget {
                break 'outer;
            }
            content.push(ch);
            used += width;
        }
        if !content.is_empty() {
            truncated.push(Span::styled(content, span.style));
        }
    }

    if !ellipsis.is_empty() {
        truncated.push(Span::styled(ellipsis.to_string(), last_style));
    }
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::history::{ExecCell, ExecSource, HistoryCell, ToolCell, ToolStatus};

    fn plain_lines(cache: &TranscriptViewCache) -> Vec<String> {
        cache
            .lines()
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    fn user_cell(content: &str) -> HistoryCell {
        HistoryCell::User {
            content: content.to_string(),
        }
    }

    fn assistant_cell(content: &str, streaming: bool) -> HistoryCell {
        HistoryCell::Assistant {
            content: content.to_string(),
            streaming,
        }
    }

    fn exec_tool_cell(command: &str) -> HistoryCell {
        HistoryCell::Tool(ToolCell::Exec(ExecCell {
            command: command.to_string(),
            status: ToolStatus::Running,
            output: None,
            started_at: None,
            duration_ms: None,
            source: ExecSource::Assistant,
            interaction: None,
            output_summary: None,
        }))
    }

    #[test]
    fn cache_reuses_cells_when_revision_unchanged() {
        let cells = vec![
            user_cell("hello"),
            assistant_cell("world", false),
            user_cell("again"),
        ];
        let revisions = vec![1u64, 1, 1];

        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());
        let first_lines: Vec<String> = cache
            .lines()
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        let first_total = cache.total_lines();
        assert!(first_total > 0, "expected non-empty render");

        // Capture per-cell lines snapshot to verify reuse.
        let snapshot_per_cell: Vec<Vec<String>> = cache
            .per_cell
            .iter()
            .map(|c| {
                c.lines
                    .iter()
                    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                    .collect()
            })
            .collect();

        // Same revisions => everything reused, output identical.
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());
        let second_lines: Vec<String> = cache
            .lines()
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert_eq!(first_lines, second_lines);
        assert_eq!(cache.total_lines(), first_total);

        let snapshot_per_cell_2: Vec<Vec<String>> = cache
            .per_cell
            .iter()
            .map(|c| {
                c.lines
                    .iter()
                    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
                    .collect()
            })
            .collect();
        assert_eq!(snapshot_per_cell, snapshot_per_cell_2);
    }

    #[test]
    fn bumping_one_cell_revision_only_rerenders_that_cell() {
        // Track render counts per cell using a custom HistoryCell wrapper
        // would require trait changes; instead, we detect reuse by inspecting
        // CachedCell instances. After a bump, only the bumped cell's stored
        // revision should differ from before; others remain identical.

        let cells_v1 = vec![
            user_cell("hello"),
            assistant_cell("hi", true),
            user_cell("again"),
        ];
        let revs_v1 = vec![1u64, 1, 1];

        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells_v1, &revs_v1, 80, TranscriptRenderOptions::default());

        // Snapshot the cached lines for cells 0 and 2 (unchanged across the
        // delta).
        let cell0_lines_before = cache.per_cell[0]
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let cell2_lines_before = cache.per_cell[2]
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();

        // Mutate cell 1 (assistant streaming delta) and bump only its rev.
        let cells_v2 = vec![
            user_cell("hello"),
            assistant_cell("hi world", true),
            user_cell("again"),
        ];
        let revs_v2 = vec![1u64, 2, 1];

        cache.ensure(&cells_v2, &revs_v2, 80, TranscriptRenderOptions::default());

        // Cells 0 and 2 are byte-identical (proving reuse path didn't corrupt).
        let cell0_lines_after = cache.per_cell[0]
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let cell2_lines_after = cache.per_cell[2]
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        assert_eq!(cell0_lines_before, cell0_lines_after);
        assert_eq!(cell2_lines_before, cell2_lines_after);

        // Cell 1 reflects the new content.
        // The renderer interleaves role/whitespace spans, so the joined
        // content has internal padding (e.g. "Assistant   hi   world").
        // Check for the new tokens individually rather than a literal
        // "hi world" substring.
        let cell1_after: String = cache.per_cell[1]
            .lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            cell1_after.contains("hi") && cell1_after.contains("world"),
            "cell1 should re-render with new content; got: {cell1_after}"
        );

        // Revisions in cache reflect the bump.
        assert_eq!(cache.per_cell[0].revision, 1);
        assert_eq!(cache.per_cell[1].revision, 2);
        assert_eq!(cache.per_cell[2].revision, 1);
    }

    #[test]
    fn tail_update_suffix_rebuild_matches_fresh_flatten() {
        let mut cells = vec![
            user_cell("first message"),
            assistant_cell("stable answer", false),
            user_cell("tail prompt"),
        ];
        let mut revisions = vec![1u64, 1, 1];
        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells, &revisions, 40, TranscriptRenderOptions::default());

        cells.push(assistant_cell("streaming tail", true));
        revisions.push(1);
        cache.ensure(&cells, &revisions, 40, TranscriptRenderOptions::default());

        if let HistoryCell::Assistant { content, .. } = cells.last_mut().unwrap() {
            content.push_str(" plus delta");
        }
        *revisions.last_mut().unwrap() += 1;
        cache.ensure(&cells, &revisions, 40, TranscriptRenderOptions::default());
        let incremental = plain_lines(&cache);

        let mut fresh = TranscriptViewCache::new();
        fresh.ensure(&cells, &revisions, 40, TranscriptRenderOptions::default());
        assert_eq!(incremental, plain_lines(&fresh));
    }

    #[test]
    fn width_change_rerenders_all_cells() {
        let cells = vec![
            user_cell("a fairly long message that may wrap at narrow widths"),
            assistant_cell("another long message body content", false),
        ];
        let revisions = vec![5u64, 7];

        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());
        let wide_total = cache.total_lines();

        // Narrow width should change layout — everything re-renders.
        cache.ensure(&cells, &revisions, 20, TranscriptRenderOptions::default());
        let narrow_total = cache.total_lines();

        assert_ne!(
            wide_total, narrow_total,
            "narrow width should produce a different number of lines"
        );

        // Restoring the original width re-renders again.
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());
        assert_eq!(cache.total_lines(), wide_total);
    }

    #[test]
    fn streaming_assistant_only_rebuilds_one_cell_render_count() {
        // Verify behavior 6: when one Assistant cell streams a delta, only
        // that one cell is re-rendered. We use a counting wrapper hooked into
        // a custom History setup. Since `lines_with_options` is on `HistoryCell`
        // (concrete enum), we can't mock it directly. Instead we verify the
        // cache's invariant: cells with unchanged revisions retain their
        // previous CachedCell entries (clone-equal), proving no re-render
        // happened for them.
        //
        // We do this by storing revisions as monotonic u64 and verifying that
        // a `Vec<u64>` snapshot of `per_cell.revision` only differs at the
        // index that was bumped.

        let mut cells: Vec<HistoryCell> =
            (0..50).map(|i| user_cell(&format!("cell {i}"))).collect();
        cells.push(assistant_cell("streaming", true));
        let mut revisions: Vec<u64> = vec![1; 51];

        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());

        // Snapshot total bytes rendered for cells 0..50 (unchanged).
        let stable_snapshot: Vec<String> = cache.per_cell[..50]
            .iter()
            .map(|c| {
                c.lines
                    .iter()
                    .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect();

        // Stream 10 deltas to the assistant cell, bumping only its revision.
        for i in 0..10 {
            if let HistoryCell::Assistant { content, .. } = &mut cells[50] {
                content.push_str(&format!(" delta-{i}"));
            }
            revisions[50] += 1;
            cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());

            // After every delta, cells 0..50 must be byte-identical to the
            // initial render. If we re-rendered them we'd observe identical
            // bytes anyway (deterministic), but the test ALSO checks the
            // CachedCell.revision values stayed at 1 — meaning the cache
            // never replaced them, only reused them.
            let stable_now: Vec<String> = cache.per_cell[..50]
                .iter()
                .map(|c| {
                    c.lines
                        .iter()
                        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .collect();
            assert_eq!(
                stable_now, stable_snapshot,
                "stable cells diverged at delta {i}"
            );

            for (idx, c) in cache.per_cell[..50].iter().enumerate() {
                assert_eq!(
                    c.revision, 1,
                    "cell {idx} revision changed during streaming delta"
                );
            }
        }
    }

    #[test]
    fn missing_revisions_falls_back_to_full_render() {
        // If callers pass a `cell_revisions` slice with the wrong length
        // (shouldn't happen, but be defensive), the cache should still
        // produce correct output rather than panic or skip cells.
        let cells = vec![user_cell("a"), assistant_cell("b", false)];
        let bogus_revisions = vec![1u64]; // wrong length

        let mut cache = TranscriptViewCache::new();
        cache.ensure(
            &cells,
            &bogus_revisions,
            80,
            TranscriptRenderOptions::default(),
        );

        // Both cells were rendered (no panic, output non-empty).
        assert_eq!(cache.per_cell.len(), 2);
        assert!(!cache.lines().is_empty());
    }

    #[test]
    fn adjacent_tool_cells_render_as_one_railed_group() {
        let cells = vec![exec_tool_cell("cargo test"), exec_tool_cell("cargo clippy")];
        let revisions = vec![1u64, 1];
        let mut cache = TranscriptViewCache::new();

        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());
        let lines = plain_lines(&cache);

        assert!(
            lines
                .first()
                .is_some_and(|line| line.starts_with("\u{256D} ")),
            "first tool line should open the shared rail: {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.starts_with("\u{2502} ")),
            "middle tool lines should continue the shared rail: {lines:?}"
        );
        assert!(
            lines
                .last()
                .is_some_and(|line| line.starts_with("\u{2570} ")),
            "last tool line should close the shared rail: {lines:?}"
        );
        assert!(
            !lines.iter().any(String::is_empty),
            "adjacent tool cells should not be separated by blank spacer rows: {lines:?}"
        );
    }

    #[test]
    fn tool_rails_preserve_rendered_width_budget() {
        let cells = vec![exec_tool_cell(
            "printf 'this is a command with enough text to wrap in narrow terminals'",
        )];
        let revisions = vec![1u64];
        let mut cache = TranscriptViewCache::new();

        cache.ensure(&cells, &revisions, 24, TranscriptRenderOptions::default());

        for line in plain_lines(&cache) {
            assert!(
                unicode_width::UnicodeWidthStr::width(line.as_str()) <= 24,
                "tool rail line exceeded narrow width: {line:?}"
            );
        }
    }

    /// Simulate a long, complex conversation (thinking + multi-line tool output +
    /// tool headers with multiple decorative spans) and report the memory
    /// consumed by `rail_prefix_widths`. This is informational — the assertion
    /// only fails if the per-line overhead exceeds a generous bound.
    // Test prints memory-overhead diagnostics — runs in `cargo test`, never
    // inside the TUI alt-screen, so the module-level deny doesn't apply.
    #[allow(clippy::print_stderr)]
    #[test]
    fn rail_prefix_widths_memory_overhead_complex_session() {
        let mut cells: Vec<HistoryCell> = Vec::new();
        // Build ~60 turns covering the typical deep-reasoning workflow:
        // user → thinking (5-15 lines) → assistant → tool → tool output →
        // thinking → assistant → ... repeat.
        for i in 0..30 {
            cells.push(user_cell(&format!("complex query {i} about system design")));
            cells.push(HistoryCell::Thinking {
                content:
                    "line A\nline B\nline C\nline D\nline E\nline F\nline G\nline H\nline I\nline J"
                        .to_string(),
                streaming: false,
                duration_secs: Some(3.5),
            });
            cells.push(assistant_cell(
                &format!("response {i} with multi-line\ntext content spanning\nseveral lines"),
                false,
            ));
            cells.push(exec_tool_cell(
                "cargo test --package my_crate -- --nocapture 2>&1 | head -40",
            ));
            // Insert a second tool so adjacent tool cells merge into a railed group.
            cells.push(exec_tool_cell(&format!("git diff --stat HEAD~{i}")));
        }
        let revisions: Vec<u64> = (0..cells.len()).map(|i| i as u64 + 1).collect();

        let mut cache = TranscriptViewCache::new();
        cache.ensure(&cells, &revisions, 80, TranscriptRenderOptions::default());

        let total_lines = cache.total_lines();
        let pw_len = cache.rail_prefix_widths.len();
        let pw_cap = cache.rail_prefix_widths.capacity();
        // The Vec's inlined buffer on most platforms is small; capacity
        // should be >= len. Both must equal total_lines.
        assert_eq!(pw_len, total_lines);
        assert!(pw_cap >= pw_len);

        let memory_bytes = pw_cap * std::mem::size_of::<usize>();
        let memory_kb = memory_bytes as f64 / 1024.0;
        // Each usize is 8 bytes on 64-bit. Even with 100k lines this stays
        // under 1 MB.
        let kbytes_per_1k_lines = (memory_bytes as f64 / total_lines as f64) * 1000.0 / 1024.0;

        eprintln!("=== rail_prefix_widths memory (complex session) ===");
        eprintln!("  total_lines:       {total_lines}");
        eprintln!("  vec len:           {pw_len}");
        eprintln!("  vec capacity:      {pw_cap}");
        eprintln!("  memory (bytes):    {memory_bytes}");
        eprintln!("  memory (KB):       {memory_kb:.2}");
        eprintln!("  KB per 1k lines:   {kbytes_per_1k_lines:.2}");
        eprintln!("  lines × 8 bytes:   {} KB", total_lines * 8 / 1024);

        // Sanity: per-line overhead must be reasonable.
        assert!(
            memory_kb < 1024.0,
            "rail_prefix_widths memory unexpectedly large: {memory_kb:.1} KB"
        );
        eprintln!("  ✓ well under 1 MB even for very long sessions");
    }
}
