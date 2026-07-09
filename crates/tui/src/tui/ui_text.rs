//! Shared text helpers for TUI selection and clipboard workflows.

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::history::HistoryCell;
use crate::tui::osc8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CopyLineSeparator {
    None,
    Space,
    Newline,
}

impl CopyLineSeparator {
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "",
            Self::Space => " ",
            Self::Newline => "\n",
        }
    }
}

pub(crate) fn truncate_line_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    // For very small budgets, take chars until we exceed the *display* width.
    if max_width <= 3 {
        let mut out = String::new();
        let mut width = 0usize;
        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if width + ch_width > max_width {
                break;
            }
            out.push(ch);
            width += ch_width;
        }
        return out;
    }

    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

/// Truncate `text` to `max_width` display columns, preferring whole words.
pub(crate) fn semantic_truncate(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if text_display_width(text) <= max_width {
        return text.to_string();
    }

    const ELLIPSIS: char = '…';
    let ellipsis_width = char_display_width(ELLIPSIS);
    let limit = max_width.saturating_sub(ellipsis_width);
    if limit == 0 {
        return ELLIPSIS.to_string();
    }

    let mut width = 0usize;
    let mut cut_byte = 0usize;
    let mut last_word_end = None;
    let mut in_word = false;
    for (byte_idx, ch) in text.char_indices() {
        let ch_width = char_display_width(ch);
        if width + ch_width > limit {
            break;
        }
        width += ch_width;
        cut_byte = byte_idx + ch.len_utf8();
        if ch.is_whitespace() {
            if in_word {
                last_word_end = Some(byte_idx);
                in_word = false;
            }
        } else {
            in_word = true;
        }
    }
    if cut_byte == 0 {
        return ELLIPSIS.to_string();
    }

    let mut body = if let Some(word_end) = last_word_end {
        text[..word_end].trim_end()
    } else {
        text[..cut_byte].trim_end()
    };
    if body.is_empty() {
        body = text[..cut_byte].trim_end();
    }
    let mut out = body.to_string();
    out.push(ELLIPSIS);
    out
}

pub(crate) fn semantic_truncate_with_affixes(
    prefix: &str,
    text: &str,
    suffix: &str,
    max_width: usize,
) -> String {
    let fixed_width = text_display_width(prefix) + text_display_width(suffix);
    if fixed_width > max_width {
        return semantic_truncate(&format!("{prefix}{text}{suffix}"), max_width);
    }
    format!(
        "{prefix}{}{suffix}",
        semantic_truncate_between_affixes(prefix, text, suffix, max_width)
    )
}

pub(crate) fn semantic_truncate_between_affixes(
    prefix: &str,
    text: &str,
    suffix: &str,
    max_width: usize,
) -> String {
    let fixed_width = text_display_width(prefix) + text_display_width(suffix);
    if fixed_width > max_width {
        return String::new();
    }
    semantic_truncate(text, max_width - fixed_width)
}

pub(crate) fn concise_shell_command_label(command: &str, max_width: usize) -> String {
    let normalized = normalize_shell_text(command);
    if let Some(label) = gh_command_label(&normalized) {
        return truncate_line_to_width(&label, max_width);
    }

    let segment = actionable_shell_segment(&normalized).unwrap_or_else(|| normalized.clone());
    truncate_line_to_width(&segment, max_width)
}

fn normalize_shell_text(text: &str) -> String {
    let mut cleaned = String::with_capacity(text.len());
    crate::tui::osc8::strip_ansi_into(text, &mut cleaned);
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn actionable_shell_segment(command: &str) -> Option<String> {
    command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace('|', "\n")
        .split(['\n', ';'])
        .map(str::trim)
        .find(|segment| {
            !segment.is_empty()
                && !segment.starts_with("cd ")
                && !segment.starts_with("sleep ")
                && !segment.starts_with("export ")
                && *segment != "true"
                && *segment != ":"
        })
        .map(str::to_string)
}

fn gh_command_label(command: &str) -> Option<String> {
    let tokens: Vec<String> = command
        .split_whitespace()
        .map(|token| {
            token
                .trim_matches(|ch: char| matches!(ch, '\'' | '"' | '(' | ')' | ';' | ','))
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .collect();

    for index in 0..tokens.len() {
        let token = tokens[index].as_str();
        if token != "gh" && !token.ends_with("/gh") {
            continue;
        }
        let Some(area) = tokens.get(index + 1).map(String::as_str) else {
            continue;
        };
        let Some(action) = tokens.get(index + 2).map(String::as_str) else {
            continue;
        };
        if !matches!(area, "pr" | "run") {
            continue;
        }
        if !matches!(
            action,
            "checks" | "view" | "status" | "list" | "watch" | "rerun"
        ) {
            continue;
        }

        let mut label = format!("gh {area} {action}");
        if let Some(target) = tokens
            .iter()
            .skip(index + 3)
            .map(String::as_str)
            .find(|token| !token.starts_with('-') && *token != "&&" && *token != ";")
        {
            label.push(' ');
            label.push_str(target);
        }
        return Some(label);
    }
    None
}

pub(super) fn history_cell_to_text(cell: &HistoryCell, width: u16) -> String {
    cell.transcript_lines(width)
        .into_iter()
        .map(line_to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_to_string(line: Line<'static>) -> String {
    let mut out = String::new();
    append_spans_plain(line.spans.iter(), &mut out);
    out
}

/// Convert a rendered transcript line to plain text, stripping OSC-8 link
/// escape sequences. The caller is responsible for shifting selection columns
/// to account for any visual-only rail prefix (see
/// `TranscriptViewCache::rail_prefix_width`).
pub(super) fn line_to_plain(line: &Line<'static>) -> String {
    let mut out = String::new();
    append_spans_plain(line.spans.iter(), &mut out);
    out
}

fn append_spans_plain<'a, I>(spans: I, out: &mut String)
where
    I: Iterator<Item = &'a Span<'a>>,
{
    for span in spans {
        if span.content.contains('\x1b') {
            osc8::strip_into(&span.content, out);
        } else {
            out.push_str(span.content.as_ref());
        }
    }
}

pub(crate) fn text_display_width(text: &str) -> usize {
    text.chars().map(char_display_width).sum()
}

pub(super) fn slice_text(text: &str, start: usize, end: usize) -> String {
    if end <= start {
        return String::new();
    }

    let mut out = String::new();
    let mut col = 0usize;
    for ch in text.chars() {
        let ch_width = char_display_width(ch);
        let ch_start = col;
        let ch_end = col.saturating_add(ch_width);
        if ch_end > start && ch_start < end {
            out.push(ch);
        }
        col = ch_end;
        if col >= end {
            break;
        }
    }
    out
}

pub(super) fn char_display_width(ch: char) -> usize {
    if ch == '\t' {
        4
    } else {
        // `width()` returns `None` for control/unassigned chars (default them to
        // one column so layout doesn't collapse) and `Some(0)` for genuinely
        // zero-width chars — combining marks, ZWJ, zero-width spaces — which must
        // stay 0 so display-width math (truncation, slicing, overflow, copy)
        // matches what the terminal actually renders.
        UnicodeWidthChar::width(ch).unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Span;

    #[test]
    fn line_to_plain_strips_osc_8_wrapper() {
        let wrapped = format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            "https://example.com", "https://example.com"
        );
        let line = Line::from(vec![
            Span::raw("see "),
            Span::raw(wrapped),
            Span::raw(" for details"),
        ]);
        let text = line_to_plain(&line);
        assert_eq!(text, "see https://example.com for details");
    }

    #[test]
    fn line_to_plain_passes_through_plain_spans() {
        let line = Line::from(vec![Span::raw("plain "), Span::raw("text")]);
        let text = line_to_plain(&line);
        assert_eq!(text, "plain text");
    }

    #[test]
    fn line_to_plain_includes_all_spans() {
        // Visual-only rail spans are stripped by the caller using
        // TranscriptViewCache::rail_prefix_width — line_to_plain itself
        // is a faithful span-to-string pass-through.
        let line = Line::from(vec![Span::raw("\u{2502} "), Span::raw("tool output")]);
        let text = line_to_plain(&line);
        assert_eq!(text, "\u{2502} tool output");
    }

    #[test]
    fn slice_text_respects_column_bounds() {
        let text = "hello world";
        assert_eq!(slice_text(text, 0, 5), "hello");
        assert_eq!(slice_text(text, 6, 11), "world");
        assert_eq!(slice_text(text, 0, 0), "");
        assert_eq!(slice_text(text, 0, 100), text);
    }

    #[test]
    fn slice_text_handles_multibyte_characters() {
        let text = "a─b"; // U+2500 is 1 display column on supported terminals
        assert_eq!(slice_text(text, 1, 2), "─");
        assert_eq!(slice_text(text, 0, 3), text);
    }

    #[test]
    fn slice_text_truncates_at_end() {
        let text = "ab";
        assert_eq!(slice_text(text, 1, 5), "b");
    }

    // --- Unicode / CJK / terminal-width QA (issue #3488) -------------------
    // These exercise the production width helpers directly so the assertions
    // track the same code path the renderer uses.

    #[test]
    fn text_display_width_counts_cjk_as_two_columns() {
        assert_eq!(text_display_width("中文"), 4); // two wide glyphs
        assert_eq!(text_display_width("Hello世界"), 9); // 5 ASCII + 2×2
        // Full-width (ambiguous→wide) punctuation is two columns each.
        assert_eq!(text_display_width("，。！？"), 8);
    }

    #[test]
    fn text_display_width_treats_zero_width_marks_as_zero() {
        // A combining mark adds no column: "e" + U+0301 renders as one cell.
        // (Regression guard: the old `.max(1)` counted it as 1, over-reporting
        // width and causing premature truncation / border drift on text with
        // combining marks or ZWJ emoji sequences.)
        assert_eq!(text_display_width("e\u{0301}"), 1);
        assert_eq!(text_display_width("cafe\u{0301}"), 4);
        // ZWJ joiner itself is zero-width; the two emoji are 2 cols each.
        assert_eq!(text_display_width("\u{1F469}\u{200D}\u{1F4BB}"), 4);
    }

    #[test]
    fn text_display_width_keeps_control_and_tab_widths() {
        // Control chars still occupy a column (avoid layout collapse); tab = 4.
        assert_eq!(text_display_width("a\u{0007}b"), 3);
        assert_eq!(text_display_width("\t"), 4);
        assert_eq!(text_display_width("\ta"), 5);
    }

    #[test]
    fn truncate_line_to_width_respects_display_width_not_byte_len() {
        // No truncation when the string already fits by display width.
        assert_eq!(truncate_line_to_width("中文", 10), "中文");
        // Oversized: reserve 3 cols for the ellipsis, fill the rest by width.
        let out = truncate_line_to_width("中文测试", 7);
        assert_eq!(out, "中文...");
        assert_eq!(text_display_width(&out), 7);
        // Never split a wide glyph across the boundary, and never emit U+FFFD.
        let clipped = truncate_line_to_width("界界界界界", 5);
        assert!(text_display_width(&clipped) <= 5);
        assert!(!clipped.contains('\u{FFFD}'));
    }

    #[test]
    fn semantic_truncate_prefers_word_boundaries() {
        let out = semantic_truncate("hello world foo bar", 14);
        assert_eq!(out, "hello world…");
        assert!(text_display_width(&out) <= 14);
    }

    #[test]
    fn semantic_truncate_falls_back_with_long_words_and_wide_glyphs() {
        let long_word = semantic_truncate("supercalifragilistic", 8);
        assert_eq!(long_word, "superca…");
        assert!(text_display_width(&long_word) <= 8);

        let cjk = semantic_truncate("中文测试文本", 7);
        assert_eq!(cjk, "中文测…");
        assert!(text_display_width(&cjk) <= 7);
    }

    #[test]
    fn semantic_truncate_handles_empty_and_tiny_budgets() {
        assert_eq!(semantic_truncate("", 10), "");
        assert_eq!(semantic_truncate("hello", 0), "");
        assert_eq!(semantic_truncate("hello", 1), "…");
    }

    #[test]
    fn semantic_truncate_between_affixes_reserves_fixed_columns() {
        let hint = semantic_truncate_between_affixes(
            " > [ ] Prefix stability  (",
            "whether system/tools stayed cacheable",
            ")",
            49,
        );
        let row = format!(" > [ ] Prefix stability  ({hint})");
        assert_eq!(hint, "whether system/tools…");
        assert!(text_display_width(&row) <= 49);
    }

    #[test]
    fn slice_text_slices_cjk_by_display_column() {
        // Columns:  中=[0,2) 文=[2,4) a=[4,5) b=[5,6)
        let text = "中文ab";
        assert_eq!(slice_text(text, 0, 2), "中");
        assert_eq!(slice_text(text, 2, 4), "文");
        assert_eq!(slice_text(text, 4, 6), "ab");
    }

    #[test]
    fn concise_shell_command_label_prefers_gh_pr_checks_over_wrappers() {
        let label = concise_shell_command_label(
            "cd /tmp/repo && sleep 15 && gh pr checks 1611 --repo Hmbown/CodeWhale",
            80,
        );
        assert_eq!(label, "gh pr checks 1611");
    }

    #[test]
    fn concise_shell_command_label_falls_back_to_actionable_segment() {
        let label = concise_shell_command_label("cd /tmp/repo && cargo test --workspace", 80);
        assert_eq!(label, "cargo test --workspace");
    }

    #[test]
    fn concise_shell_command_label_strips_ansi_before_collapsing_text() {
        let label = concise_shell_command_label(
            "cd /repo && \x1b[38;2;6;174;242mcargo test\x1b[0m --workspace",
            80,
        );
        assert_eq!(label, "cargo test --workspace");
        assert!(!label.contains("38;2"));
    }

    // --- New #3488 fixtures: CJK/wide-glyph truncation on selector-style rows.
    // truncate_line_to_width is the production helper behind sidebar (file_tree),
    // statusline (footer_ui), hotbar, and picker (mouse_ui) row rendering, so
    // these exercise the same truncation path those surfaces use.

    #[test]
    fn truncate_line_to_width_full_width_cjk_lands_on_glyph_boundary() {
        // Each Han glyph is two columns. With an odd budget the truncation must
        // land on a whole-glyph boundary (reserving three columns for the
        // ellipsis), never leaving a half-rendered wide cell or emitting U+FFFD.
        let title = "项目报告结果"; // 6 glyphs, 12 columns
        let out = truncate_line_to_width(title, 7);
        // Budget 7 -> limit 4 columns -> two glyphs fit, then the ellipsis.
        assert_eq!(out, "项目...");
        assert_eq!(text_display_width(&out), 7);
        // The kept prefix is composed only of whole wide glyphs (each 2 cols),
        // proving the boundary glyph was dropped whole, not split.
        let prefix = out.strip_suffix("...").expect("ellipsis present");
        assert!(prefix.chars().all(|c| char_display_width(c) == 2));
        assert!(!out.contains('\u{FFFD}'));
    }

    #[test]
    fn truncate_line_to_width_mixed_ascii_cjk_row_keeps_ellipsis_within_budget() {
        // A sidebar/selector row mixing an ASCII label with a CJK title, wider
        // than the column budget, must truncate with a trailing ellipsis that
        // still fits by display width and must not split a wide glyph.
        let row = "Task: 数据库迁移任务 done"; // ASCII label + 7 Han glyphs
        let budget = 12;
        let out = truncate_line_to_width(row, budget);
        assert!(out.ends_with("..."), "expected ellipsis, got {out:?}");
        // Ellipsis-and-content fit within the budget by *display* width.
        assert!(text_display_width(&out) <= budget);
        // The non-ellipsis prefix stays within budget-minus-ellipsis, so the
        // wide glyph on the boundary was dropped whole rather than half-drawn.
        let prefix = out.strip_suffix("...").expect("ellipsis present");
        assert!(text_display_width(prefix) <= budget - 3);
        assert!(!out.contains('\u{FFFD}'));
        // The semantic ASCII prefix survives truncation.
        assert!(out.starts_with("Task:"));
    }

    #[test]
    fn truncate_line_to_width_dense_cjk_selector_row_survives_narrow_widths() {
        // Picker/selector rows degrade through truncate_line_to_width when the
        // terminal is narrow. A dense row with a leading marker glyph and CJK
        // content must stay within budget at tiny widths, without panicking or
        // emitting a replacement char from a mid-glyph byte split.
        let row = "▸ 中文项目 · main"; // marker + CJK + separator + branch
        for width in [1usize, 2, 3, 4, 6, 8] {
            let out = truncate_line_to_width(row, width);
            assert!(
                text_display_width(&out) <= width,
                "width={width}: {out:?} exceeds budget"
            );
            assert!(
                !out.contains('\u{FFFD}'),
                "width={width}: truncation split a wide glyph"
            );
        }
    }
}
