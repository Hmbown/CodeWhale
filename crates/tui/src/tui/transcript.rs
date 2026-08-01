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

use std::collections::HashSet;
use std::sync::Arc;

use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::tui::app::TranscriptSpacing;
use crate::tui::history::{HistoryCell, TranscriptRenderOptions};
use crate::tui::scrolling::TranscriptLineMeta;
use crate::tui::ui_text::CopyLineSeparator;

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
#[derive(Debug)]
struct CachedCell {
    /// Revision the cell was at when the lines/meta were rendered.
    revision: u64,
    /// Rendered lines for this cell (without trailing inter-cell spacers),
    /// shared via `Arc` so cache enumeration is O(N) not O(N*lines).
    lines: Arc<Vec<Line<'static>>>,
    /// Hyperlinks aligned with `lines`, in display columns relative to each
    /// line. Targets never enter the ratatui cell buffer.
    links: Arc<Vec<Vec<crate::tui::osc8::LineLink>>>,
    /// Copy separators aligned with `lines`. These preserve source hard
    /// newlines while allowing copy to remove visual soft-wrap breaks.
    copy_separators: Arc<Vec<CopyLineSeparator>>,
    /// Display-column widths of visual prefixes that should be omitted from
    /// clipboard text, aligned with `lines`.
    copy_prefix_widths: Arc<Vec<usize>>,
    /// Whether this cell's rendered output was empty (e.g. Thinking hidden).
    /// Cached so we can skip empty cells without re-rendering.
    is_empty: bool,
    /// Semantic role used by the transcript's explicit boundary matrix.
    /// Keeping the role in the cache makes spacing independent of rendered
    /// strings, theme colors, terminal depth, and animation state.
    kind: TranscriptBlockKind,
    /// Whether this cell participates in the compact tool-card rail group.
    is_tool_groupable: bool,
    /// Persistent parser/highlighter carry for the one changing Assistant
    /// cell. Stable rendered lines remain in the vectors above and are
    /// truncated only from the cache's replaceable-tail index.
    incremental_markdown: Option<Box<crate::tui::markdown_render::IncrementalMarkdownRenderCache>>,
    /// The hot-tail treatment mutates the last line for animation. Preserve
    /// its settled form so the next append can restore it without re-rendering
    /// the stable prefix.
    hot_tail_original: Option<(usize, Line<'static>)>,
}

/// Provenance that one live Assistant cell's source stayed unchanged or only
/// gained appended bytes. Visual-only revision bumps can therefore reuse it.
/// Revisions use the same transformed keys passed to `ensure_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamingSourceReceipt {
    pub cell_index: usize,
    pub from_revision: u64,
    pub to_revision: u64,
    pub content_len: usize,
}

/// Visual role of one transcript cell.
///
/// Approval, question, Work-panel, and composer surfaces live outside the
/// transcript cache and already own bounded panels/edges. This enum covers
/// every in-transcript seam, including durable Work receipts emitted by plan,
/// checklist, and workflow tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptBlockKind {
    User,
    Reasoning,
    Answer,
    ToolAction,
    DurableWork,
    Notice,
}

impl TranscriptBlockKind {
    fn for_cell(cell: &HistoryCell) -> Self {
        match cell {
            HistoryCell::User { .. } => Self::User,
            HistoryCell::Thinking { .. } => Self::Reasoning,
            HistoryCell::Assistant { .. } => Self::Answer,
            HistoryCell::Tool(tool) if tool.is_durable_work_receipt() => Self::DurableWork,
            HistoryCell::Tool(_) | HistoryCell::SubAgent(_) => Self::ToolAction,
            HistoryCell::System { .. }
            | HistoryCell::Error { .. }
            | HistoryCell::ArchivedContext { .. } => Self::Notice,
        }
    }
}

/// Strength of a visible boundary. These three levels are the complete
/// transcript spacing vocabulary: no blanket per-cell padding is added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptBoundary {
    /// Two cells are one response/activity group.
    Joined,
    /// Compact transition into or out of tools, Work, or notices.
    Activity,
    /// A human turn boundary; always visible, even at compact density.
    Turn,
}

/// Cache of rendered transcript lines for the current viewport.
#[derive(Debug)]
pub struct TranscriptViewCache {
    width: u16,
    options: TranscriptRenderOptions,
    /// Snapshot of folded_thinking indices from the last `ensure` call.
    /// When this changes, all cells must be re-rendered because the fold
    /// state affects the rendered output but not the cell revision.
    folded_cells: HashSet<usize>,
    /// Per-cell rendered output, indexed by current cell position.
    /// Length always equals the cell count seen on the last `ensure` call.
    per_cell: Vec<CachedCell>,
    /// Flattened lines reassembled from `per_cell` plus spacers.
    lines: Vec<Line<'static>>,
    /// Per-line hyperlink metadata aligned with `lines`.
    line_links: Vec<Vec<crate::tui::osc8::LineLink>>,
    /// Per-line metadata aligned with `lines`.
    line_meta: Vec<TranscriptLineMeta>,
    /// Per-line rail-prefix display-column count (`0` or `2`), aligned with
    /// `lines`. Populated during flatten so that selection-to-text can shift
    /// columns past visual-only decoration glyphs without guessing which
    /// spans are decorative (#1163).
    rail_prefix_widths: Vec<usize>,
    streaming_source_receipt: Option<StreamingSourceReceipt>,
    /// Deterministic receipt for actual flattened-line reconstruction work.
    /// Kept in production state so tests measure the real path without hooks.
    streaming_lines_reflattened: u64,
    streaming_meta_rows_scanned: u64,
}

impl TranscriptViewCache {
    /// Create an empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self {
            width: 0,
            options: TranscriptRenderOptions::default(),
            folded_cells: HashSet::new(),
            per_cell: Vec::new(),
            lines: Vec::new(),
            line_links: Vec::new(),
            line_meta: Vec::new(),
            rail_prefix_widths: Vec::new(),
            streaming_source_receipt: None,
            streaming_lines_reflattened: 0,
            streaming_meta_rows_scanned: 0,
        }
    }

    pub(crate) fn set_streaming_source_receipt(&mut self, receipt: Option<StreamingSourceReceipt>) {
        self.streaming_source_receipt = receipt;
    }

    #[cfg(test)]
    #[must_use]
    fn streaming_lines_reflattened(&self) -> u64 {
        self.streaming_lines_reflattened
    }

    #[cfg(test)]
    #[must_use]
    fn streaming_meta_rows_scanned(&self) -> u64 {
        self.streaming_meta_rows_scanned
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
        self.ensure_split(
            &[cells],
            cell_revisions,
            width,
            options,
            &HashSet::new(),
            None,
        );
    }

    /// Ensure cached lines match the provided cell shards (logically
    /// concatenated) plus per-cell revisions. Avoids the
    /// `concat-into-Vec<HistoryCell>` clone the caller would otherwise pay
    /// every frame on long transcripts.
    ///
    /// `folded_cells` contains original virtual indices of thinking cells
    /// that should render in their folded (summary) form.
    ///
    /// `original_index_map` maps filtered (positional) indices to original
    /// virtual indices. Required when `collapsed_cells` filtering is active
    /// so that `folded_cells` lookups resolve to the correct original index.
    pub fn ensure_split(
        &mut self,
        cell_shards: &[&[HistoryCell]],
        cell_revisions: &[u64],
        width: u16,
        options: TranscriptRenderOptions,
        folded_cells: &HashSet<usize>,
        original_index_map: Option<&[usize]>,
    ) {
        let total_cells: usize = cell_shards.iter().map(|s| s.len()).sum();
        self.ensure_iter(
            total_cells,
            cell_shards.iter().flat_map(|shard| shard.iter()),
            cell_revisions,
            width,
            options,
            folded_cells,
            original_index_map,
        );
    }

    /// `ensure_split` over an already-filtered list of borrowed cells.
    ///
    /// The collapse path substitutes synthetic tool-run summary cells and
    /// skips collapsed cells, so it cannot hand over contiguous shard
    /// slices. Accepting `&[&HistoryCell]` lets it pass borrows instead of
    /// deep-cloning every visible cell into a fresh `Vec<HistoryCell>` each
    /// frame (#3896).
    #[allow(clippy::too_many_arguments)]
    pub fn ensure_filtered(
        &mut self,
        cells: &[&HistoryCell],
        cell_revisions: &[u64],
        width: u16,
        options: TranscriptRenderOptions,
        folded_cells: &HashSet<usize>,
        original_index_map: Option<&[usize]>,
    ) {
        self.ensure_iter(
            cells.len(),
            cells.iter().copied(),
            cell_revisions,
            width,
            options,
            folded_cells,
            original_index_map,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn ensure_iter<'a>(
        &mut self,
        total_cells: usize,
        cells: impl Iterator<Item = &'a HistoryCell>,
        cell_revisions: &[u64],
        width: u16,
        options: TranscriptRenderOptions,
        folded_cells: &HashSet<usize>,
        original_index_map: Option<&[usize]>,
    ) {
        let layout_changed = self.width != width || self.options != options;
        let folded_changed = self.folded_cells != *folded_cells;
        if layout_changed || folded_changed {
            self.per_cell.clear();
        }
        self.width = width;
        self.options = options;
        self.folded_cells = folded_cells.clone();

        // Track whether anything actually changed; if all cells are reused at
        // the same indices, we can skip the reflatten.
        let old_len = self.per_cell.len();
        let mut any_dirty = layout_changed || folded_changed || old_len != total_cells;
        let mut first_dirty: Option<usize> = if old_len != total_cells {
            Some(old_len.min(total_cells))
        } else {
            None
        };

        let mut old_per_cell: Vec<Option<CachedCell>> = std::mem::take(&mut self.per_cell)
            .into_iter()
            .map(Some)
            .collect();
        let mut new_per_cell: Vec<CachedCell> = Vec::with_capacity(total_cells);
        let revisions_match = cell_revisions.len() == total_cells;
        let mut dirty_cells = 0usize;
        let mut streaming_tail_update = None;

        let mut idx: usize = 0;
        for cell in cells {
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
            if !layout_changed
                && revisions_match
                && old_per_cell
                    .get(idx)
                    .and_then(Option::as_ref)
                    .is_some_and(|prev| prev.revision == current_rev)
            {
                new_per_cell.push(
                    old_per_cell[idx]
                        .take()
                        .expect("cached cell checked as present"),
                );
                idx += 1;
                continue;
            }

            any_dirty = true;
            dirty_cells = dirty_cells.saturating_add(1);
            first_dirty = Some(first_dirty.map_or(idx, |current| current.min(idx)));
            let is_tool_groupable = matches!(cell, HistoryCell::Tool(_));
            let render_width = if is_tool_groupable {
                width.saturating_sub(2).max(1)
            } else {
                width
            };
            let original_idx = original_index_map
                .map(|m| *m.get(idx).unwrap_or(&idx))
                .unwrap_or(idx);
            let folded = folded_cells.contains(&original_idx);

            if matches!(
                cell,
                HistoryCell::Assistant {
                    streaming: true,
                    ..
                }
            ) {
                let mut cached = old_per_cell
                    .get_mut(idx)
                    .and_then(Option::take)
                    .unwrap_or_else(|| CachedCell {
                        revision: current_rev,
                        lines: Arc::new(Vec::new()),
                        links: Arc::new(Vec::new()),
                        copy_separators: Arc::new(Vec::new()),
                        copy_prefix_widths: Arc::new(Vec::new()),
                        is_empty: true,
                        kind: TranscriptBlockKind::Answer,
                        is_tool_groupable: false,
                        incremental_markdown: Some(Box::default()),
                        hot_tail_original: None,
                    });
                if let Some((line_index, original)) = cached.hot_tail_original.take()
                    && let Some(line) = Arc::make_mut(&mut cached.lines).get_mut(line_index)
                {
                    *line = original;
                }
                let content_len = match cell {
                    HistoryCell::Assistant { content, .. } => content.len(),
                    _ => 0,
                };
                let verified_append = self.streaming_source_receipt.is_some_and(|receipt| {
                    receipt.cell_index == original_idx
                        && receipt.from_revision == cached.revision
                        && receipt.to_revision == current_rev
                        && receipt.content_len == content_len
                });
                let incremental = cached.incremental_markdown.get_or_insert_with(Box::default);
                let replace_from = cell
                    .update_incremental_streaming_render(
                        render_width,
                        options,
                        verified_append,
                        incremental,
                        Arc::make_mut(&mut cached.lines),
                        Arc::make_mut(&mut cached.links),
                        Arc::make_mut(&mut cached.copy_separators),
                        Arc::make_mut(&mut cached.copy_prefix_widths),
                    )
                    .expect("streaming Assistant matched above");
                let cached_lines = Arc::make_mut(&mut cached.lines);
                let last_index = cached_lines.len().checked_sub(1);
                if let Some((index, last)) = last_index
                    .and_then(|index| cached_lines.get_mut(index).map(|line| (index, line)))
                {
                    cached.hot_tail_original = Some((index, last.clone()));
                    crate::tui::history::apply_hot_tail_to_line(last, options.low_motion);
                }
                cached.revision = current_rev;
                cached.is_empty = cached.lines.is_empty();
                cached.kind = TranscriptBlockKind::Answer;
                cached.is_tool_groupable = false;
                // The hot-tail style also changes on the preceding settled
                // line, so reflatten one line before the Markdown tail.
                streaming_tail_update = Some((idx, replace_from.saturating_sub(1)));
                new_per_cell.push(cached);
                idx += 1;
                continue;
            }

            let rendered = cell.lines_with_copy_metadata_folded(render_width, options, folded);
            let mut lines = Vec::with_capacity(rendered.len());
            let mut links = Vec::with_capacity(rendered.len());
            let mut copy_separators = Vec::with_capacity(rendered.len());
            let mut copy_prefix_widths = Vec::with_capacity(rendered.len());
            for rendered_line in rendered {
                let mut line = rendered_line.line;
                if is_tool_groupable {
                    strip_cell_local_tool_rail(&mut line);
                }
                lines.push(line);
                links.push(rendered_line.links);
                copy_prefix_widths.push(rendered_line.copy_prefix_width);
                copy_separators.push(rendered_line.copy_separator_after);
            }
            let is_empty = lines.is_empty();
            new_per_cell.push(CachedCell {
                revision: current_rev,
                lines: Arc::new(lines),
                links: Arc::new(links),
                copy_separators: Arc::new(copy_separators),
                copy_prefix_widths: Arc::new(copy_prefix_widths),
                is_empty,
                kind: TranscriptBlockKind::for_cell(cell),
                is_tool_groupable,
                incremental_markdown: None,
                hot_tail_original: None,
            });
            idx += 1;
        }

        self.per_cell = new_per_cell;

        if !any_dirty {
            // All cells reused at the same indices: nothing to reflatten.
            // (Width didn't change either, since that bumps `layout_changed`.)
            return;
        }

        if !layout_changed
            && !folded_changed
            && old_len == total_cells
            && dirty_cells == 1
            && let Some((cell_index, line_from)) = streaming_tail_update
            && cell_index + 1 == total_cells
            && self.flatten_streaming_tail(cell_index, line_from)
        {
            return;
        }

        let mut rebuild_from = if layout_changed {
            0
        } else {
            first_dirty.unwrap_or(0).saturating_sub(1)
        };
        // A hidden cell has no line at which `flatten_from` can truncate.
        // Walk back to the nearest visible predecessor so a cell appearing,
        // disappearing, or changing kind cannot leave a stale spacer behind.
        while rebuild_from > 0
            && self
                .per_cell
                .get(rebuild_from)
                .is_some_and(|cell| cell.is_empty)
        {
            rebuild_from -= 1;
        }
        self.flatten_from(options.spacing, rebuild_from);
    }

    /// Reassemble flat `lines` / `line_meta` from `per_cell` plus spacers.
    fn flatten(&mut self, spacing: TranscriptSpacing) {
        self.lines.clear();
        self.line_links.clear();
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
        self.line_links.truncate(truncate_at);
        self.line_meta.truncate(truncate_at);
        self.rail_prefix_widths.truncate(truncate_at);
        self.append_flattened_cells(spacing, first_cell);
    }

    /// Replace only the changing tail of the final streaming cell in the
    /// flattened viewport. Returns false when the prior cell had no visible
    /// line at the requested boundary, in which case the caller performs the
    /// canonical suffix rebuild.
    fn flatten_streaming_tail(&mut self, cell_index: usize, line_from: usize) -> bool {
        // Search backward: for append-only updates `line_from` is at the old
        // hot tail, so this examines only the replaceable suffix rather than
        // the full transcript prefix.
        let mut truncate_at = None;
        for (index, meta) in self.line_meta.iter().enumerate().rev() {
            self.streaming_meta_rows_scanned = self.streaming_meta_rows_scanned.saturating_add(1);
            if matches!(
                meta,
                TranscriptLineMeta::CellLine {
                    cell_index: candidate,
                    line_in_cell,
                    ..
                } if *candidate == cell_index && *line_in_cell == line_from
            ) {
                truncate_at = Some(index);
                break;
            }
        }
        let Some(truncate_at) = truncate_at else {
            return false;
        };
        self.lines.truncate(truncate_at);
        self.line_links.truncate(truncate_at);
        self.line_meta.truncate(truncate_at);
        self.rail_prefix_widths.truncate(truncate_at);

        let Some(cached) = self.per_cell.get(cell_index) else {
            return false;
        };
        let rendered_line_count = cached.lines.len();
        for line_in_cell in line_from..rendered_line_count {
            let line = &cached.lines[line_in_cell];
            let rail = tool_group_rail(
                self.per_cell.as_slice(),
                cell_index,
                line_in_cell,
                rendered_line_count,
            );
            let final_line = line_with_group_rail(line, rail, usize::from(self.width));
            let final_links = links_with_group_rail(
                cached.links.get(line_in_cell).map_or(&[], Vec::as_slice),
                rail,
                usize::from(self.width),
            );
            self.rail_prefix_widths
                .push(compute_rail_prefix_width(&final_line));
            self.lines.push(final_line);
            self.line_links.push(final_links);
            self.line_meta.push(TranscriptLineMeta::CellLine {
                cell_index,
                line_in_cell,
                copy_prefix_width: cached
                    .copy_prefix_widths
                    .get(line_in_cell)
                    .copied()
                    .unwrap_or(0),
                copy_separator_after: cached
                    .copy_separators
                    .get(line_in_cell)
                    .copied()
                    .unwrap_or(CopyLineSeparator::Newline),
            });
            self.streaming_lines_reflattened = self.streaming_lines_reflattened.saturating_add(1);
        }
        true
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
                let rail = tool_group_rail(
                    self.per_cell.as_slice(),
                    cell_index,
                    line_in_cell,
                    rendered_line_count,
                );
                let final_line = line_with_group_rail(line, rail, usize::from(self.width));
                let final_links = links_with_group_rail(
                    cached.links.get(line_in_cell).map_or(&[], Vec::as_slice),
                    rail,
                    usize::from(self.width),
                );
                self.rail_prefix_widths
                    .push(compute_rail_prefix_width(&final_line));
                self.lines.push(final_line);
                self.line_links.push(final_links);
                self.line_meta.push(TranscriptLineMeta::CellLine {
                    cell_index,
                    line_in_cell,
                    copy_prefix_width: cached
                        .copy_prefix_widths
                        .get(line_in_cell)
                        .copied()
                        .unwrap_or(0),
                    copy_separator_after: cached
                        .copy_separators
                        .get(line_in_cell)
                        .copied()
                        .unwrap_or(CopyLineSeparator::Newline),
                });
                self.streaming_lines_reflattened =
                    self.streaming_lines_reflattened.saturating_add(1);
            }

            if let Some(next) = next_visible_cell(&self.per_cell, cell_index) {
                let spacer_rows = spacer_rows_between(cached, next, spacing);
                for _ in 0..spacer_rows {
                    self.lines.push(Line::from(""));
                    self.line_links.push(Vec::new());
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

    /// Return hyperlinks aligned with [`Self::lines`].
    #[must_use]
    pub fn line_links(&self) -> &[Vec<crate::tui::osc8::LineLink>] {
        &self.line_links
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

/// Tool cells still render their own rail when used outside the transcript
/// cache (pager, clipboard, focused detail). Inside the live transcript this
/// cache owns grouping across adjacent cells, so retaining both rails produces
/// doubled prefixes such as `╭ ╭`. Replace the cell-local decoration with the
/// group rail added by `line_with_group_rail` during flattening.
fn strip_cell_local_tool_rail(line: &mut Line<'static>) {
    if line
        .spans
        .first()
        .is_some_and(|span| matches!(span.content.as_ref(), "─ " | "╭ " | "│ " | "╰ "))
    {
        line.spans.remove(0);
    }
}

fn spacer_rows_between(
    current: &CachedCell,
    next: &CachedCell,
    spacing: TranscriptSpacing,
) -> usize {
    spacer_rows_for_boundary(
        transcript_boundary(
            current.kind,
            next.kind,
            same_tool_activity_group(current, next),
        ),
        spacing,
    )
}

/// Adjacent tool cells share one rail only when they represent the same kind
/// of activity. Durable Work receipts are persisted state, not another
/// transient action, so crossing that semantic seam closes the current rail
/// even at compact density where no blank row is available.
fn same_tool_activity_group(current: &CachedCell, next: &CachedCell) -> bool {
    current.is_tool_groupable && next.is_tool_groupable && current.kind == next.kind
}

fn transcript_boundary(
    current: TranscriptBlockKind,
    next: TranscriptBlockKind,
    same_tool_group: bool,
) -> TranscriptBoundary {
    if same_tool_group {
        debug_assert_eq!(current, next);
        return TranscriptBoundary::Joined;
    }

    // A user block is the only unambiguous turn delimiter available to the
    // renderer. Keep it distinct from direct tool execution too: models may
    // legitimately move from a prompt straight into a tool without first
    // emitting answer prose.
    if current == TranscriptBlockKind::User || next == TranscriptBlockKind::User {
        return TranscriptBoundary::Turn;
    }

    // Reasoning and answer prose are phases of one model response. Joining
    // them also keeps the row budget stable when streaming reasoning settles
    // into the final answer.
    if matches!(
        (current, next),
        (
            TranscriptBlockKind::Reasoning | TranscriptBlockKind::Answer,
            TranscriptBlockKind::Reasoning | TranscriptBlockKind::Answer
        )
    ) {
        return TranscriptBoundary::Joined;
    }

    TranscriptBoundary::Activity
}

const fn spacer_rows_for_boundary(
    boundary: TranscriptBoundary,
    spacing: TranscriptSpacing,
) -> usize {
    match (boundary, spacing) {
        (TranscriptBoundary::Joined, _) => 0,
        (TranscriptBoundary::Activity, TranscriptSpacing::Compact) => 0,
        (TranscriptBoundary::Activity, _) => 1,
        (TranscriptBoundary::Turn, TranscriptSpacing::Compact | TranscriptSpacing::Comfortable) => {
            1
        }
        (TranscriptBoundary::Turn, TranscriptSpacing::Spacious) => 2,
    }
}

fn previous_visible_cell(cells: &[CachedCell], cell_index: usize) -> Option<&CachedCell> {
    cells[..cell_index].iter().rev().find(|cell| !cell.is_empty)
}

fn next_visible_cell(cells: &[CachedCell], cell_index: usize) -> Option<&CachedCell> {
    cells
        .get(cell_index + 1..)?
        .iter()
        .find(|cell| !cell.is_empty)
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

    let previous_shares_group = previous_visible_cell(cells, cell_index)
        .is_some_and(|previous| same_tool_activity_group(previous, cached));
    let next_shares_group = next_visible_cell(cells, cell_index)
        .is_some_and(|next| same_tool_activity_group(cached, next));
    let first_line_in_group = !previous_shares_group && line_in_cell == 0;
    let last_line_in_group = !next_shares_group && line_in_cell + 1 == rendered_line_count;

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

fn links_with_group_rail(
    links: &[crate::tui::osc8::LineLink],
    rail: Option<crate::tui::widgets::tool_card::CardRail>,
    max_width: usize,
) -> Vec<crate::tui::osc8::LineLink> {
    let shift = rail
        .map(crate::tui::widgets::tool_card::rail_glyph)
        .filter(|glyph| !glyph.is_empty())
        .map_or(0, |glyph| unicode_width::UnicodeWidthStr::width(glyph) + 1);
    links
        .iter()
        .map(|link| link.shifted(shift))
        .filter(|link| link.col_start < max_width)
        .map(|mut link| {
            link.col_end = link.col_end.min(max_width.saturating_sub(1));
            link
        })
        .collect()
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
#[path = "transcript/tests.rs"]
mod tests;
