//! Shared text helpers for TUI selection and clipboard workflows.

use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::tui::history::HistoryCell;
use crate::tui::osc8;

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

pub(super) fn text_display_width(text: &str) -> usize {
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

fn char_display_width(ch: char) -> usize {
    if ch == '\t' {
        4
    } else {
        UnicodeWidthChar::width(ch).unwrap_or(0).max(1)
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
}
