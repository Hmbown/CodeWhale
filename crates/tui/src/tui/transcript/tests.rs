use super::*;
use crate::localization::Locale;
use crate::palette;
use crate::tools::plan::PlanSnapshot;
use crate::tui::history::{
    ExecCell, ExecSource, HistoryCell, PlanUpdateCell, ReasoningAction, ReasoningActionTarget,
    ToolCell, ToolStatus, TranscriptActionOwner,
};

impl TranscriptViewCache {
    pub(crate) fn reasoning_action_target(&self) -> Option<ReasoningActionTarget> {
        self.reasoning_action_target
    }

    fn streaming_lines_reflattened(&self) -> u64 {
        self.streaming_lines_reflattened
    }

    fn streaming_meta_rows_scanned(&self) -> u64 {
        self.streaming_meta_rows_scanned
    }
}

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

fn reasoning_cell(streaming: bool) -> HistoryCell {
    HistoryCell::Thinking {
        content: (1..=20)
            .map(|line| format!("reasoning line {line:02}"))
            .collect::<Vec<_>>()
            .join("\n"),
        streaming,
        duration_secs: (!streaming).then_some(1.0),
    }
}

fn reasoning_owner(cell_index: usize) -> TranscriptActionOwner {
    TranscriptActionOwner {
        cell_index,
        identity_epoch: 7,
    }
}

fn exec_tool_cell_with_output(command: &str, output: String) -> HistoryCell {
    // A failed shell cell keeps its full output in the live render, so
    // this fixture proves tool cells do not inherit the prose measure.
    HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: command.to_string(),
        status: ToolStatus::Failed,
        output: Some(output),
        live_output: None,
        shell_task_id: None,
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: None,
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    }))
}

fn exec_tool_cell(command: &str) -> HistoryCell {
    HistoryCell::Tool(ToolCell::Exec(ExecCell {
        command: command.to_string(),
        status: ToolStatus::Running,
        output: None,
        live_output: None,
        shell_task_id: None,
        owner_agent_id: None,
        owner_agent_name: None,
        started_at: None,
        duration_ms: None,
        stale_elapsed_since_output_ms: None,
        source: ExecSource::Assistant,
        interaction: None,
        output_summary: None,
    }))
}

fn durable_work_cell() -> HistoryCell {
    HistoryCell::Tool(ToolCell::PlanUpdate(PlanUpdateCell {
        snapshot: PlanSnapshot::default(),
        status: ToolStatus::Running,
    }))
}

fn spacer_rows_after_cell(cache: &TranscriptViewCache, target_cell: usize) -> usize {
    let mut saw_target = false;
    let mut spacer_rows = 0;
    for meta in cache.line_meta() {
        match meta {
            TranscriptLineMeta::CellLine { cell_index, .. } if *cell_index == target_cell => {
                saw_target = true;
                spacer_rows = 0;
            }
            TranscriptLineMeta::Spacer { .. } if saw_target => spacer_rows += 1,
            TranscriptLineMeta::CellLine { .. } if saw_target => break,
            TranscriptLineMeta::Spacer { .. } | TranscriptLineMeta::CellLine { .. } => {}
        }
    }
    spacer_rows
}

#[test]
fn cache_renders_user_cells_with_highlight_background() {
    let cells = vec![user_cell("# literal user prompt")];
    let revisions = vec![1u64];

    let mut cache = TranscriptViewCache::new();
    cache.ensure(&cells, &revisions, 40, TranscriptRenderOptions::default());

    let lines = cache.lines();
    assert_eq!(lines[0].style.bg, Some(palette::SURFACE_ELEVATED));
    assert_eq!(lines[0].width(), 40);
    assert_eq!(plain_lines(&cache)[0].trim_end(), "▎ # literal user prompt");
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
fn streaming_assistant_keeps_a_persistent_linear_render_prefix() {
    let mut content = String::new();
    let mut revision = 1u64;
    let mut cache = TranscriptViewCache::new();
    let options = TranscriptRenderOptions {
        low_motion: true,
        ..TranscriptRenderOptions::default()
    };

    content.push_str("start\n```rust\nlet value_0 = 0;\n```\n\n");
    let mut cells = vec![assistant_cell(&content, true)];
    cache.ensure(&cells, &[revision], 96, options);
    let lines_arc = Arc::as_ptr(&cache.per_cell[0].lines);

    for index in 1..120usize {
        let previous = revision;
        revision += 1;
        content.push_str(&format!(
            "段落 {index} e\u{301} 🚀\n```rust\nlet value_{index} = {index};\n```\n\n"
        ));
        cells[0] = assistant_cell(&content, true);
        cache.set_streaming_source_receipt(Some(StreamingSourceReceipt {
            cell_index: 0,
            from_revision: previous,
            to_revision: revision,
            content_len: content.len(),
        }));
        cache.ensure(&cells, &[revision], 96, options);
    }

    let previous = revision;
    revision += 1;
    cache.set_streaming_source_receipt(Some(StreamingSourceReceipt {
        cell_index: 0,
        from_revision: previous,
        to_revision: revision,
        content_len: content.len(),
    }));
    cache.ensure(&cells, &[revision], 96, options);

    assert_eq!(Arc::as_ptr(&cache.per_cell[0].lines), lines_arc);
    let work = cache.per_cell[0]
        .incremental_markdown
        .as_ref()
        .expect("streaming markdown cache")
        .work();
    assert_eq!(work.invalidations, 1);
    assert_eq!(work.tail_blocks_rendered, 0);
    assert_eq!(work.classified_lines as usize, content.lines().count());
    assert!(
        cache.streaming_lines_reflattened() <= (cache.total_lines() + 121) as u64,
        "flatten work must be final output plus at most one hot-tail line per update: work={}, final={}",
        cache.streaming_lines_reflattened(),
        cache.total_lines()
    );
    assert!(
        cache.streaming_meta_rows_scanned() <= 121,
        "reverse lookup must inspect only the replaceable tail: {}",
        cache.streaming_meta_rows_scanned()
    );

    let mut cold = TranscriptViewCache::new();
    cold.ensure(&cells, &[revision], 96, options);
    assert_eq!(plain_lines(&cache), plain_lines(&cold));
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

    let mut cells: Vec<HistoryCell> = (0..50).map(|i| user_cell(&format!("cell {i}"))).collect();
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
    // Live foreground exec cells collapse to a single header line (copy
    // dedupe #17), so a third cell is needed for a rail-continuation row.
    let cells = vec![
        exec_tool_cell("cargo test"),
        exec_tool_cell("cargo clippy"),
        exec_tool_cell("cargo fmt"),
    ];
    let revisions = vec![1u64, 1, 1];
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
        "adjacent tool cells must never be separated by a bare blank row — that \
         would tear the card box open: {lines:?}"
    );
    assert!(
        !lines.iter().any(|line| line.trim_end() == "\u{2502}"),
        "one tool group must stay compact instead of padding every call: {lines:?}"
    );
    assert_eq!(spacer_rows_after_cell(&cache, 0), 0);
    assert_eq!(spacer_rows_after_cell(&cache, 1), 0);
}

#[test]
fn semantic_boundary_matrix_has_four_deliberate_rhythm_levels() {
    use TranscriptBlockKind::{Answer, DurableWork, Notice, Reasoning, ToolAction, User};
    use TranscriptBoundary::{Activity, GroupedTool, Joined, Turn};

    let cases = [
        (User, Answer, false, Turn),
        (User, ToolAction, false, Turn),
        (DurableWork, User, false, Turn),
        // Reasoning handing off to the answer is a phase change the reader
        // has to see. Running the two together with no blank row is the
        // density complaint this matrix exists to answer.
        (Reasoning, Answer, false, Activity),
        (Answer, Reasoning, false, Activity),
        // Successive cells of the *same* phase are one block split across
        // cells; a blank row there would jitter mid-stream.
        (Answer, Answer, false, Joined),
        (Reasoning, Reasoning, false, Joined),
        (Answer, ToolAction, false, Activity),
        (ToolAction, Reasoning, false, Activity),
        (Notice, DurableWork, false, Activity),
        (ToolAction, ToolAction, true, GroupedTool),
        (DurableWork, DurableWork, true, GroupedTool),
        (ToolAction, DurableWork, false, Activity),
    ];

    for (current, next, grouped_tools, expected) in cases {
        assert_eq!(
            transcript_boundary(current, next, grouped_tools),
            expected,
            "{current:?} -> {next:?}"
        );
    }

    assert_eq!(
        spacer_rows_for_boundary(Turn, TranscriptSpacing::Compact),
        1
    );
    assert_eq!(
        spacer_rows_for_boundary(Turn, TranscriptSpacing::Comfortable),
        1
    );
    assert_eq!(
        spacer_rows_for_boundary(Turn, TranscriptSpacing::Spacious),
        2
    );
    assert_eq!(
        spacer_rows_for_boundary(Activity, TranscriptSpacing::Compact),
        0
    );
    assert_eq!(
        spacer_rows_for_boundary(Activity, TranscriptSpacing::Comfortable),
        1
    );
    assert_eq!(
        spacer_rows_for_boundary(Activity, TranscriptSpacing::Spacious),
        1
    );
    assert_eq!(
        spacer_rows_for_boundary(GroupedTool, TranscriptSpacing::Compact),
        0,
        "compact density buys its density by spending no separator rows"
    );
    assert_eq!(
        spacer_rows_for_boundary(GroupedTool, TranscriptSpacing::Comfortable),
        0,
        "the shared rail carries grouping without a row per tool call"
    );
    assert_eq!(
        spacer_rows_for_boundary(GroupedTool, TranscriptSpacing::Spacious),
        0,
        "even spacious mode breathes around the group, not inside it"
    );
}

/// Separation is one row or none. Nothing in the matrix may produce a
/// double blank, because a scrolling terminal cannot afford it.
#[test]
fn no_boundary_ever_spends_more_than_one_row_below_spacious_turns() {
    use TranscriptBoundary::{Activity, GroupedTool, Joined, Turn};

    for boundary in [Joined, GroupedTool, Activity, Turn] {
        for spacing in [
            TranscriptSpacing::Compact,
            TranscriptSpacing::Comfortable,
            TranscriptSpacing::Spacious,
        ] {
            let rows = spacer_rows_for_boundary(boundary, spacing);
            let allowed = if boundary == Turn && spacing == TranscriptSpacing::Spacious {
                2
            } else {
                BLOCK_SEPARATOR_ROWS
            };
            assert!(
                rows <= allowed,
                "{boundary:?} at {spacing:?} spent {rows} rows (max {allowed})"
            );
        }
    }
}

#[test]
fn durable_work_tools_have_an_explicit_semantic_role() {
    let plan = durable_work_cell();
    let tool = exec_tool_cell("cargo test --locked");

    assert_eq!(
        TranscriptBlockKind::for_cell(&plan),
        TranscriptBlockKind::DurableWork
    );
    assert_eq!(
        TranscriptBlockKind::for_cell(&tool),
        TranscriptBlockKind::ToolAction
    );
}

#[test]
fn durable_work_starts_a_new_activity_rail_without_wasting_compact_rows() {
    let durable = HistoryCell::Tool(ToolCell::PlanUpdate(PlanUpdateCell {
        snapshot: PlanSnapshot {
            objective: Some("Keep the release receipt durable".to_string()),
            ..PlanSnapshot::default()
        },
        status: ToolStatus::Running,
    }));
    let cells = vec![
        exec_tool_cell("cargo test --locked"),
        exec_tool_cell("cargo clippy --locked"),
        durable,
    ];
    let revisions = vec![1u64; cells.len()];

    let mut compact = TranscriptViewCache::new();
    compact.ensure(
        &cells,
        &revisions,
        80,
        TranscriptRenderOptions {
            spacing: TranscriptSpacing::Compact,
            low_motion: true,
            ..TranscriptRenderOptions::default()
        },
    );

    assert_eq!(spacer_rows_after_cell(&compact, 0), 0);
    assert_eq!(spacer_rows_after_cell(&compact, 1), 0);
    let compact_lines = plain_lines(&compact);
    assert!(
        !compact_lines.iter().any(String::is_empty),
        "compact activity seams must not spend a blank row: {compact_lines:?}"
    );
    let lines_for_cell = |target| {
        compact
            .lines()
            .iter()
            .zip(compact.line_meta())
            .filter_map(|(line, meta)| match meta {
                TranscriptLineMeta::CellLine { cell_index, .. } if *cell_index == target => Some(
                    line.spans
                        .iter()
                        .map(|span| span.content.as_ref())
                        .collect::<String>(),
                ),
                TranscriptLineMeta::Spacer { .. } | TranscriptLineMeta::CellLine { .. } => None,
            })
            .collect::<Vec<_>>()
    };
    let second_action = lines_for_cell(1);
    let durable_work = lines_for_cell(2);
    assert!(
        second_action
            .last()
            .is_some_and(|line| line.starts_with("\u{2570} ")),
        "ordinary action rail should close before durable Work: {second_action:?}"
    );
    assert!(
        durable_work
            .first()
            .is_some_and(|line| line.starts_with("\u{256D} ")),
        "durable Work should open its own rail: {durable_work:?}"
    );

    let mut comfortable = TranscriptViewCache::new();
    comfortable.ensure(
        &cells,
        &revisions,
        80,
        TranscriptRenderOptions {
            spacing: TranscriptSpacing::Comfortable,
            low_motion: true,
            ..TranscriptRenderOptions::default()
        },
    );
    assert_eq!(
        spacer_rows_after_cell(&comfortable, 0),
        0,
        "two commands inside one activity rail must remain compact"
    );
    assert_eq!(
        spacer_rows_after_cell(&comfortable, 1),
        1,
        "durable Work needs a semantic activity row outside compact density"
    );
}

#[test]
fn compact_spacing_keeps_conversation_blocks_separate() {
    let cells = vec![
        user_cell("Please verify the release."),
        assistant_cell("I will check the receipts.", false),
    ];
    let revisions = vec![1u64, 1];
    let mut cache = TranscriptViewCache::new();
    let options = TranscriptRenderOptions {
        spacing: TranscriptSpacing::Compact,
        ..TranscriptRenderOptions::default()
    };

    cache.ensure(&cells, &revisions, 89, options);
    let lines = plain_lines(&cache);

    assert!(
        lines.iter().any(String::is_empty),
        "compact density still needs one user/assistant boundary: {lines:?}"
    );
}

#[test]
fn compact_spacing_keeps_direct_user_tool_turns_separate() {
    let cells = vec![
        user_cell("Inspect the repository."),
        exec_tool_cell("git status --short"),
        user_cell("Now summarize the result."),
    ];
    let revisions = vec![1u64, 1, 1];
    let options = TranscriptRenderOptions {
        spacing: TranscriptSpacing::Compact,
        low_motion: true,
        ..TranscriptRenderOptions::default()
    };
    let mut cache = TranscriptViewCache::new();

    cache.ensure(&cells, &revisions, 80, options);

    assert_eq!(spacer_rows_after_cell(&cache, 0), 1);
    assert_eq!(spacer_rows_after_cell(&cache, 1), 1);
}

#[test]
fn compact_spacing_keeps_reasoning_and_answer_in_one_response_block() {
    let cells = vec![
        HistoryCell::Thinking {
            content: "I should verify the release receipts first.".to_string(),
            streaming: false,
            duration_secs: Some(0.4),
        },
        assistant_cell("The release receipts are green.", false),
    ];
    let revisions = vec![1u64, 1];
    let mut cache = TranscriptViewCache::new();
    let options = TranscriptRenderOptions {
        spacing: TranscriptSpacing::Compact,
        ..TranscriptRenderOptions::default()
    };

    cache.ensure(&cells, &revisions, 89, options);
    let lines = plain_lines(&cache);

    assert!(
        !lines.iter().any(String::is_empty),
        "reasoning and its answer should read as one response block: {lines:?}"
    );
}

#[test]
fn hidden_reasoning_keeps_visible_rhythm_without_phantom_tail_rows() {
    let cells = vec![
        user_cell("Verify the release."),
        HistoryCell::Thinking {
            content: "Check the exact receipts.".to_string(),
            streaming: false,
            duration_secs: Some(0.4),
        },
        assistant_cell("The receipts are green.", false),
    ];
    let revisions = vec![1u64, 1, 1];
    let hidden = TranscriptRenderOptions {
        show_thinking: false,
        low_motion: true,
        ..TranscriptRenderOptions::default()
    };
    let mut cache = TranscriptViewCache::new();

    cache.ensure(&cells, &revisions, 80, hidden);
    let hidden_lines = plain_lines(&cache);
    assert_eq!(spacer_rows_after_cell(&cache, 0), 1);
    assert!(
        hidden_lines.last().is_some_and(|line| !line.is_empty()),
        "hidden cells must not leave a trailing blank row: {hidden_lines:?}"
    );

    let visible = TranscriptRenderOptions {
        show_thinking: true,
        ..hidden
    };
    cache.ensure(&cells, &revisions, 80, visible);
    cache.ensure(&cells, &revisions, 80, hidden);
    assert_eq!(plain_lines(&cache), hidden_lines);

    let trailing_hidden = &cells[..2];
    let mut tail_cache = TranscriptViewCache::new();
    tail_cache.ensure(trailing_hidden, &revisions[..2], 80, hidden);
    assert!(
        plain_lines(&tail_cache)
            .last()
            .is_some_and(|line| !line.is_empty()),
        "a hidden final cell must not reserve a phantom spacer"
    );
}

#[test]
fn hidden_reasoning_cache_never_advertises_or_leaks_content() {
    for streaming in [false, true] {
        let cells = [reasoning_cell(streaming)];
        let mut cache = TranscriptViewCache::new();
        cache.ensure_split(
            &[&cells],
            &[1],
            80,
            TranscriptRenderOptions {
                show_thinking: false,
                ..TranscriptRenderOptions::default()
            },
            &HashSet::new(),
            None,
            Some(reasoning_owner(0)),
        );
        let text = plain_lines(&cache).join("\n");
        assert_eq!(cache.reasoning_action_target(), None);
        assert!(
            !text.contains("reasoning line"),
            "hidden body leaked: {text}"
        );
        assert!(!text.contains("Space:"), "hidden hint leaked: {text}");
        assert_eq!(text.contains("reasoning hidden"), streaming);
    }
}

#[test]
fn transcript_rhythm_is_width_and_reduced_motion_invariant() {
    let cells = vec![
        user_cell("Please inspect the release candidate and verify all receipts."),
        HistoryCell::Thinking {
            content: "I will inspect the source, run the checks, and compare the receipts."
                .to_string(),
            streaming: true,
            duration_secs: Some(0.8),
        },
        assistant_cell("I will start with the locked test suite.", false),
        exec_tool_cell("cargo test -p codewhale-tui --bins --locked"),
        durable_work_cell(),
        assistant_cell("The focused checks passed.", false),
        user_cell("Proceed to the final verification."),
    ];
    let revisions = vec![1u64; cells.len()];
    // user | reasoning | answer | tool | work | answer | user.
    // Every seam is one row: the reasoning→answer seam (index 1) used to be
    // the one place the transcript ran two blocks together.
    let expected = [1, 1, 1, 1, 1, 1, 0];

    for width in [40, 80, 100, 140] {
        for low_motion in [false, true] {
            let options = TranscriptRenderOptions {
                low_motion,
                spacing: TranscriptSpacing::Comfortable,
                ..TranscriptRenderOptions::default()
            };
            let mut cache = TranscriptViewCache::new();
            cache.ensure(&cells, &revisions, width, options);

            let actual =
                std::array::from_fn::<_, 7, _>(|index| spacer_rows_after_cell(&cache, index));
            assert_eq!(actual, expected, "width={width} low_motion={low_motion}");
            assert!(
                cache
                    .lines()
                    .iter()
                    .all(|line| line.width() <= usize::from(width)),
                "render exceeded width={width} low_motion={low_motion}"
            );
        }
    }
}

#[test]
fn streaming_state_transitions_do_not_move_neighbor_boundaries() {
    let mut cells = vec![
        user_cell("Inspect the candidate."),
        HistoryCell::Thinking {
            content: "Inspecting the candidate now.".to_string(),
            streaming: true,
            duration_secs: None,
        },
        exec_tool_cell("git status --short"),
        user_cell("Summarize the receipt."),
    ];
    let mut revisions = vec![1u64; cells.len()];
    let options = TranscriptRenderOptions {
        low_motion: true,
        ..TranscriptRenderOptions::default()
    };
    let mut cache = TranscriptViewCache::new();

    let boundary_rows = |cache: &TranscriptViewCache| {
        [
            spacer_rows_after_cell(cache, 0),
            spacer_rows_after_cell(cache, 1),
            spacer_rows_after_cell(cache, 2),
        ]
    };

    cache.ensure(&cells, &revisions, 80, options);
    assert_eq!(boundary_rows(&cache), [1, 1, 1]);

    cells[1] = assistant_cell("I inspected the candidate.", true);
    revisions[1] += 1;
    cache.ensure(&cells, &revisions, 80, options);
    assert_eq!(boundary_rows(&cache), [1, 1, 1]);

    cells[1] = assistant_cell("I inspected the candidate.", false);
    revisions[1] += 1;
    cache.ensure(&cells, &revisions, 80, options);
    assert_eq!(boundary_rows(&cache), [1, 1, 1]);

    let HistoryCell::Tool(ToolCell::Exec(exec)) = &mut cells[2] else {
        unreachable!("fixture is an exec tool")
    };
    exec.status = ToolStatus::Success;
    revisions[2] += 1;
    cache.ensure(&cells, &revisions, 80, options);
    assert_eq!(boundary_rows(&cache), [1, 1, 1]);
}

#[test]
fn resize_round_trip_rebuilds_the_same_semantic_rows() {
    let cells = vec![
        user_cell("A long prompt that wraps when the terminal narrows considerably."),
        exec_tool_cell("printf 'a tool receipt with a deliberately long summary'"),
        assistant_cell("A stable answer after the tool receipt.", false),
    ];
    let revisions = vec![1u64; cells.len()];
    let options = TranscriptRenderOptions {
        low_motion: true,
        ..TranscriptRenderOptions::default()
    };
    let mut cache = TranscriptViewCache::new();

    cache.ensure(&cells, &revisions, 140, options);
    let wide = plain_lines(&cache);
    cache.ensure(&cells, &revisions, 40, options);
    cache.ensure(&cells, &revisions, 140, options);

    assert_eq!(plain_lines(&cache), wide);
    assert_eq!(cache.lines().len(), cache.line_meta().len());
    assert_eq!(cache.lines().len(), cache.line_links().len());
}

#[test]
fn palette_mode_change_invalidates_cached_syntax_rendering() {
    let cells = vec![assistant_cell(
        "```rust\nfn main() { let answer = 42; }\n```",
        false,
    )];
    let revisions = [1u64];
    let mut cache = TranscriptViewCache::new();
    let dark = TranscriptRenderOptions {
        palette_mode: palette::PaletteMode::Dark,
        ..TranscriptRenderOptions::default()
    };

    cache.ensure(&cells, &revisions, 80, dark);
    let dark_lines = Arc::clone(&cache.per_cell[0].lines);

    cache.ensure(
        &cells,
        &revisions,
        80,
        TranscriptRenderOptions {
            palette_mode: palette::PaletteMode::Light,
            ..dark
        },
    );

    assert!(
        !Arc::ptr_eq(&dark_lines, &cache.per_cell[0].lines),
        "palette mode is part of TranscriptRenderOptions and must bust cached cells"
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

#[test]
fn ensure_filtered_matches_ensure_split_output() {
    let cells = vec![
        user_cell("hello"),
        assistant_cell("some **markdown** body", false),
        exec_tool_cell("cargo test"),
        user_cell("again"),
    ];
    let revisions = vec![1u64, 2, 3, 4];
    let index_map: Vec<usize> = vec![0, 1, 2, 3];
    // This test compares the two cache traversal paths, not animation.
    // Freeze live motion so a spinner tick between the two renders cannot
    // turn an equivalent layout into a timing-dependent failure.
    let options = TranscriptRenderOptions {
        low_motion: true,
        motion_mode: crate::tui::motion::MotionMode::Still,
        ..TranscriptRenderOptions::default()
    };

    let mut split_cache = TranscriptViewCache::new();
    split_cache.ensure_split(
        &[&cells],
        &revisions,
        40,
        options,
        &HashSet::new(),
        Some(&index_map),
        None,
    );

    let refs: Vec<&HistoryCell> = cells.iter().collect();
    let mut filtered_cache = TranscriptViewCache::new();
    filtered_cache.ensure_filtered(
        &refs,
        &revisions,
        40,
        options,
        &HashSet::new(),
        Some(&index_map),
        None,
    );

    assert_eq!(plain_lines(&split_cache), plain_lines(&filtered_cache));
    assert_eq!(
        split_cache.line_meta().len(),
        filtered_cache.line_meta().len()
    );
}

#[test]
fn ensure_filtered_reuses_unchanged_cells() {
    let cells = [
        user_cell("hello"),
        assistant_cell("streaming", true),
        user_cell("again"),
    ];
    let mut revisions = vec![1u64, 1, 1];
    let refs: Vec<&HistoryCell> = cells.iter().collect();

    let mut cache = TranscriptViewCache::new();
    cache.ensure_filtered(
        &refs,
        &revisions,
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        None,
        None,
    );
    let first = plain_lines(&cache);

    cache.ensure_filtered(
        &refs,
        &revisions,
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        None,
        None,
    );
    assert_eq!(first, plain_lines(&cache));
    for (idx, cached) in cache.per_cell.iter().enumerate() {
        assert_eq!(
            cached.revision, 1,
            "cell {idx} must be reused, not re-rendered"
        );
    }

    // Bump one revision: only that entry re-renders.
    revisions[1] = 2;
    cache.ensure_filtered(
        &refs,
        &revisions,
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        None,
        None,
    );
    assert_eq!(cache.per_cell[0].revision, 1);
    assert_eq!(cache.per_cell[1].revision, 2);
    assert_eq!(cache.per_cell[2].revision, 1);
}

#[test]
fn prose_cells_fill_full_width_on_ultrawide_by_default() {
    // #5436: prose (user/assistant/thinking) spends the full content width
    // on wide terminals, consistent with tool cells and the #5322
    // wide-frame decision. The old 105-column rail is gone unless
    // `transcript.prose_measure` opts back into a bounded measure. The
    // cache key stays `(CellId, fed_width, revision)` so resize keeps its
    // single-feed cost model; the per-cell measure is applied inside the
    // render entry points.
    const RETIRED_RAIL_MEASURE: usize = 105;
    let long = "ultrawide prose paragraph that wraps its words across \
                    the whole terminal canvas, repeated to guarantee \
                    several wrapped rows at any column budget, "
        .repeat(6);
    let cells = [
        user_cell(&long),
        assistant_cell(&long, false),
        HistoryCell::Thinking {
            content: long.clone(),
            streaming: false,
            duration_secs: Some(2.0),
        },
        exec_tool_cell_with_output(
            "cargo test --all",
            "long tool output that itself wraps well past the prose measure ".repeat(6),
        ),
    ];
    let refs: Vec<&HistoryCell> = cells.iter().collect();
    let revisions = vec![1u64, 2, 3, 4];
    let options = TranscriptRenderOptions {
        low_motion: true,
        motion_mode: crate::tui::motion::MotionMode::Still,
        // Expanded thinking so the reasoning body also spends the width.
        verbose: true,
        thinking_default_expanded: true,
        ..TranscriptRenderOptions::default()
    };

    let mut cache = TranscriptViewCache::new();
    cache.ensure_filtered(&refs, &revisions, 220, options, &HashSet::new(), None, None);

    for idx in 0..3 {
        let width = max_line_width(&cache.per_cell[idx].lines);
        assert!(
            width > RETIRED_RAIL_MEASURE,
            "prose cell {idx} wrapped to {width} columns — still on the retired \
                 {RETIRED_RAIL_MEASURE}-column rail",
        );
        assert!(
            width <= 220,
            "prose cell {idx} wrapped to {width} columns, past the 220-column canvas",
        );
    }
    let tool_width = max_line_width(&cache.per_cell[3].lines);
    assert!(
        tool_width > RETIRED_RAIL_MEASURE,
        "tool cell must keep the full width, got {tool_width}",
    );
}

#[test]
fn transcript_prose_measure_caps_prose_but_not_tools() {
    // A positive `transcript.prose_measure` restores a bounded reading
    // measure for prose only; tool/status cells keep the full content width
    // (#5436). The 120-column cap is deliberately above the retired
    // 105-column rail so a pass proves the configured cap — not the old
    // default — is in effect.
    let long = "ultrawide ".repeat(400);
    let cells = [
        user_cell(&long),
        assistant_cell(&long, false),
        HistoryCell::Thinking {
            content: long.clone(),
            streaming: false,
            duration_secs: Some(2.0),
        },
        exec_tool_cell_with_output(
            "cargo test --all",
            "long tool output that itself wraps well past the prose measure ".repeat(6),
        ),
    ];
    let refs: Vec<&HistoryCell> = cells.iter().collect();
    let revisions = vec![1u64, 2, 3, 4];
    let options = TranscriptRenderOptions {
        low_motion: true,
        motion_mode: crate::tui::motion::MotionMode::Still,
        verbose: true,
        thinking_default_expanded: true,
        prose_measure: Some(120),
        ..TranscriptRenderOptions::default()
    };

    let mut cache = TranscriptViewCache::new();
    cache.ensure_filtered(&refs, &revisions, 220, options, &HashSet::new(), None, None);

    for idx in 0..3 {
        let width = max_line_width(&cache.per_cell[idx].lines);
        assert!(
            width > 105,
            "prose cell {idx} wrapped to {width} columns — still on the retired \
                 105-column rail, so the configured cap is not in effect",
        );
        assert!(
            width <= 120,
            "prose cell {idx} wrapped to {width} columns, over the 120-column measure",
        );
    }
    let tool_width = max_line_width(&cache.per_cell[3].lines);
    assert!(
        tool_width > 120,
        "tool cell must keep the full width past the prose measure, got {tool_width}",
    );
}

#[test]
fn overlay_and_streaming_entries_share_the_prose_measure() {
    // The main cache and the full-screen live-transcript overlay must agree
    // on the same effective prose width — the reason the measure rides on
    // `TranscriptRenderOptions` instead of being re-derived per cell
    // (#5436). Render one streaming assistant message through both
    // live-transcript entry points: the copy-metadata path (overlay) and
    // the incremental streaming path (active cell).
    let long = "ultrawide ".repeat(400);
    let cell = assistant_cell(&long, true);
    let options = TranscriptRenderOptions {
        low_motion: true,
        motion_mode: crate::tui::motion::MotionMode::Still,
        prose_measure: Some(120),
        ..TranscriptRenderOptions::default()
    };

    let overlay_lines = cell.lines_with_copy_metadata(220, options);
    let overlay_width = overlay_lines
        .iter()
        .map(|line| max_line_width(std::slice::from_ref(&line.line)))
        .max()
        .unwrap_or(0);

    let mut cache = crate::tui::markdown_render::IncrementalMarkdownRenderCache::default();
    let mut streaming_lines = Vec::new();
    let mut links = Vec::new();
    let mut separators = Vec::new();
    let mut prefix_widths = Vec::new();
    cell.update_incremental_streaming_render(
        220,
        options,
        false,
        &mut cache,
        &mut streaming_lines,
        &mut links,
        &mut separators,
        &mut prefix_widths,
    );
    let streaming_width = max_line_width(&streaming_lines);

    for (name, width) in [("overlay", overlay_width), ("streaming", streaming_width)] {
        assert!(
            width > 105,
            "{name} entry stayed on the retired 105-column rail ({width} columns)",
        );
        assert!(
            width <= 120,
            "{name} entry wrapped to {width} columns, over the 120-column measure",
        );
    }
}

#[test]
fn prose_width_resolves_the_transcript_prose_measure_contract() {
    // Absent = full content width (floored at 1); a positive cap clamps
    // from above only, so narrow terminals keep their content width
    // (#5436: 0/absent means full width).
    let full = TranscriptRenderOptions::default();
    assert_eq!(full.prose_measure, None);
    assert_eq!(full.prose_width(220), 220);
    assert_eq!(full.prose_width(96), 96);
    assert_eq!(full.prose_width(0), 1);

    let capped = TranscriptRenderOptions {
        prose_measure: Some(120),
        ..TranscriptRenderOptions::default()
    };
    assert_eq!(capped.prose_width(220), 120);
    assert_eq!(capped.prose_width(96), 96);
    assert_eq!(capped.prose_width(0), 1);
}

fn max_line_width(lines: &[Line<'static>]) -> usize {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| unicode_width::UnicodeWidthStr::width(span.content.as_ref()))
                .sum()
        })
        .max()
        .unwrap_or(0)
}

#[test]
fn folded_thinking_cache_invalidation() {
    let long_content = "reasoning line\n".repeat(50);
    let cells = [HistoryCell::Thinking {
        content: long_content.clone(),
        streaming: false,
        duration_secs: Some(1.5),
    }];
    let revisions = [1u64];
    let options = TranscriptRenderOptions {
        verbose: true, // expanded by default
        ..TranscriptRenderOptions::default()
    };
    let width = 80u16;

    // First render: no folding → full content.
    let mut cache = TranscriptViewCache::new();
    cache.ensure_split(
        &[&cells],
        &revisions,
        width,
        options,
        &HashSet::new(),
        None,
        None,
    );
    let full_line_count = cache.total_lines();

    // Second render: fold the thinking cell → should invalidate and
    // produce fewer lines (collapsed summary).
    let mut folded = HashSet::new();
    folded.insert(0usize);
    cache.ensure_split(&[&cells], &revisions, width, options, &folded, None, None);
    let folded_line_count = cache.total_lines();

    assert!(
        folded_line_count < full_line_count,
        "folded thinking should render fewer lines: folded={folded_line_count} full={full_line_count}"
    );

    // Third render: unfold → should restore full content.
    cache.ensure_split(
        &[&cells],
        &revisions,
        width,
        options,
        &HashSet::new(),
        None,
        None,
    );
    let restored_line_count = cache.total_lines();
    assert_eq!(
        restored_line_count, full_line_count,
        "unfolded thinking should restore full line count"
    );
}

#[test]
fn folded_thinking_with_collapsed_cells_uses_original_indices() {
    // Two thinking cells: cell 0 and cell 1. Cell 0 is collapsed (hidden).
    // Fold cell 1 (original index 1). With the filtered index map,
    // the cache should still fold the correct cell.
    let cells = [
        HistoryCell::Thinking {
            content: "first thinking block\n".repeat(20),
            streaming: false,
            duration_secs: Some(1.0),
        },
        HistoryCell::Thinking {
            content: "second thinking block\n".repeat(20),
            streaming: false,
            duration_secs: Some(2.0),
        },
    ];
    let revisions = [1u64, 2u64];
    let options = TranscriptRenderOptions {
        verbose: true,
        ..TranscriptRenderOptions::default()
    };
    let width = 80u16;

    // No collapsing, no folding — baseline.
    let mut cache = TranscriptViewCache::new();
    cache.ensure_split(
        &[&cells],
        &revisions,
        width,
        options,
        &HashSet::new(),
        None,
        None,
    );
    let baseline = cache.total_lines();
    assert!(baseline > 0, "baseline render should contain visible lines");

    // Collapse cell 0, fold cell 1. The filtered list has only cell 1
    // at filtered index 0, but it maps to original index 1.
    let filtered_cells = [cells[1].clone()];
    let filtered_revs = [2u64];
    let index_map: Vec<usize> = vec![1]; // filtered 0 → original 1

    let mut folded = HashSet::new();
    folded.insert(1usize); // fold original index 1

    let mut cache2 = TranscriptViewCache::new();
    cache2.ensure_split(
        &[&filtered_cells],
        &filtered_revs,
        width,
        options,
        &folded,
        Some(&index_map),
        None,
    );
    let folded_filtered = cache2.total_lines();

    // Cell 1 was expanded in baseline; now it should be folded.
    // We can't compare directly to baseline because baseline had both
    // cells, but folded_filtered should be less than if cell 1 were
    // expanded in the filtered view.
    let mut cache3 = TranscriptViewCache::new();
    cache3.ensure_split(
        &[&filtered_cells],
        &filtered_revs,
        width,
        options,
        &HashSet::new(),
        Some(&index_map),
        None,
    );
    let expanded_filtered = cache3.total_lines();

    assert!(
        folded_filtered < expanded_filtered,
        "folded cell via index map should render fewer lines: folded={folded_filtered} expanded={expanded_filtered}"
    );
}

#[test]
fn reasoning_target_transfer_rewrites_same_revision_cells() {
    let cells = vec![reasoning_cell(false), reasoning_cell(false)];
    let revisions = [1, 2];
    let mut cache = TranscriptViewCache::new();
    let options = TranscriptRenderOptions::default();
    let hint_cells = |cache: &TranscriptViewCache| {
        cache
            .lines()
            .iter()
            .zip(cache.line_meta())
            .filter(|(line, _)| line.to_string().contains("Space:expand"))
            .filter_map(|(_, meta)| meta.cell_line().map(|(cell, _)| cell))
            .collect::<Vec<_>>()
    };

    cache.ensure_split(
        &[&cells],
        &revisions,
        80,
        options,
        &HashSet::new(),
        None,
        Some(reasoning_owner(0)),
    );
    let total = cache.total_lines();
    assert_eq!(hint_cells(&cache), vec![0]);
    let cached_lines = cache
        .per_cell
        .iter()
        .map(|cell| Arc::as_ptr(&cell.lines))
        .collect::<Vec<_>>();
    let (hint_line, hint_meta) = cache
        .lines()
        .iter()
        .zip(cache.line_meta())
        .find(|(line, _)| line.to_string().contains("Space:expand"))
        .expect("hint line");
    let hint_index = cache
        .lines()
        .iter()
        .position(|line| line.to_string().contains("Space:expand"))
        .expect("hint index");
    assert_eq!(
        hint_meta.copy_prefix_width(),
        hint_line
            .width()
            .saturating_sub(cache.rail_prefix_width(hint_index))
    );
    let (neutral_index, neutral_line, neutral_meta) = cache
        .lines()
        .iter()
        .zip(cache.line_meta())
        .enumerate()
        .find(|(_, (_, meta))| {
            meta.cell_line() == Some((1, cache.per_cell[1].lines.len().saturating_sub(1)))
        })
        .map(|(index, (line, meta))| (index, line, meta))
        .expect("untargeted neutral affordance");
    assert_eq!(
        neutral_meta.copy_prefix_width(),
        neutral_line
            .width()
            .saturating_sub(cache.rail_prefix_width(neutral_index))
    );
    assert!(cache.line_links[neutral_index].is_empty());

    cache.retarget(Some(reasoning_owner(1)), None);
    assert_eq!(cache.total_lines(), total);
    assert_eq!(hint_cells(&cache), vec![1]);
    assert_eq!(
        cache
            .per_cell
            .iter()
            .map(|cell| Arc::as_ptr(&cell.lines))
            .collect::<Vec<_>>(),
        cached_lines,
        "target transfer must reuse neutral Markdown renders"
    );

    cache.retarget(None, None);
    assert_eq!(cache.total_lines(), total);
    assert!(hint_cells(&cache).is_empty());
}

#[test]
fn filtered_reasoning_owner_keeps_original_identity() {
    let cells = [reasoning_cell(false)];
    let revisions = [2];
    let original_map = [1];
    let mut cache = TranscriptViewCache::new();
    cache.ensure_split(
        &[&cells],
        &revisions,
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        Some(&original_map),
        Some(reasoning_owner(1)),
    );
    assert!(plain_lines(&cache).join("\n").contains("Space:expand"));
    assert_eq!(
        cache.reasoning_action_target(),
        Some(ReasoningActionTarget {
            owner: reasoning_owner(1),
            action: ReasoningAction::Expand,
        })
    );
    assert!(
        cache
            .line_meta()
            .iter()
            .any(|meta| meta.cell_line().is_some_and(|(rendered, _)| rendered == 0))
    );

    cache.ensure_split(
        &[&cells],
        &revisions,
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        Some(&original_map),
        Some(reasoning_owner(0)),
    );
    assert!(cache.reasoning_action_target().is_none());
    assert!(!plain_lines(&cache).join("\n").contains("Space:expand"));
}

#[test]
fn streaming_tail_fast_path_cannot_skip_reasoning_retarget() {
    let cells = [reasoning_cell(false), assistant_cell("tail", true)];
    let mut cache = TranscriptViewCache::new();
    cache.ensure_split(
        &[&cells],
        &[1, 1],
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        None,
        Some(reasoning_owner(0)),
    );
    assert!(plain_lines(&cache).join("\n").contains("Space:expand"));

    let updated = [reasoning_cell(false), assistant_cell("tail extended", true)];
    cache.ensure_split(
        &[&updated],
        &[1, 2],
        80,
        TranscriptRenderOptions::default(),
        &HashSet::new(),
        None,
        None,
    );
    assert!(cache.reasoning_action_target().is_none());
    assert!(!plain_lines(&cache).join("\n").contains("Space:expand"));
}

#[test]
fn narrow_reasoning_hint_never_changes_cache_geometry() {
    let cells = [reasoning_cell(false)];
    for width in 1..=16 {
        let mut cache = TranscriptViewCache::new();
        cache.ensure_split(
            &[&cells],
            &[1],
            width,
            TranscriptRenderOptions::default(),
            &HashSet::new(),
            None,
            None,
        );
        let neutral_lines = cache.total_lines();
        cache.ensure_split(
            &[&cells],
            &[1],
            width,
            TranscriptRenderOptions::default(),
            &HashSet::new(),
            None,
            Some(reasoning_owner(0)),
        );
        assert_eq!(cache.total_lines(), neutral_lines, "width {width}");
        let affordance_line = cache
            .lines()
            .iter()
            .zip(cache.line_meta())
            .find(|(_, meta)| {
                meta.cell_line() == Some((0, cache.per_cell[0].lines.len().saturating_sub(1)))
            })
            .map(|(line, _)| line.to_string())
            .expect("reasoning affordance line");
        if width >= 14 {
            assert_eq!(affordance_line, "╎ Space:expand", "width {width}");
        } else {
            assert_eq!(affordance_line, "╎ …", "width {width}");
            assert!(!plain_lines(&cache).join("\n").contains("Space:"));
        }
    }
}

#[test]
fn reasoning_hint_uses_the_render_locale() {
    let cells = [reasoning_cell(false)];
    let options = TranscriptRenderOptions {
        locale: Locale::Ja,
        ..TranscriptRenderOptions::default()
    };
    let mut cache = TranscriptViewCache::new();
    cache.ensure_split(
        &[&cells],
        &[1],
        80,
        options,
        &HashSet::new(),
        None,
        Some(reasoning_owner(0)),
    );
    let text = plain_lines(&cache).join("\n");
    assert!(text.contains("Space:展開"), "{text}");
    assert!(!text.contains("Space:expand"), "{text}");
}
