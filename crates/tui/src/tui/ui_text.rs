//! Shared text helpers for TUI selection and clipboard workflows.

use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::localization::Locale;
use crate::tui::history::HistoryCell;
use crate::tui::osc8;

pub(super) fn history_cell_to_text(cell: &HistoryCell, width: u16, locale: Locale) -> String {
    cell.transcript_lines_with_locale(width, locale)
        .into_iter()
        .map(line_to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_to_string(line: Line<'static>) -> String {
    let mut out = String::new();
    for span in line.spans {
        if span.content.contains('\x1b') {
            osc8::strip_into(&span.content, &mut out);
        } else {
            out.push_str(&span.content);
        }
    }
    out
}

pub(super) fn line_to_plain(line: &Line<'static>) -> String {
    let mut out = String::new();
    for span in &line.spans {
        if span.content.contains('\x1b') {
            osc8::strip_into(&span.content, &mut out);
        } else {
            out.push_str(span.content.as_ref());
        }
    }
    out
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
        // A span carrying an OSC 8-wrapped URL must not leak the escape into
        // selection / clipboard output. The visible label survives.
        let wrapped = format!(
            "\x1b]8;;{}\x1b\\{}\x1b]8;;\x1b\\",
            "https://example.com", "https://example.com"
        );
        let line = Line::from(vec![
            Span::raw("see "),
            Span::raw(wrapped),
            Span::raw(" for details"),
        ]);
        assert_eq!(line_to_plain(&line), "see https://example.com for details");
    }

    #[test]
    fn line_to_plain_passes_through_plain_spans() {
        let line = Line::from(vec![Span::raw("plain "), Span::raw("text")]);
        assert_eq!(line_to_plain(&line), "plain text");
    }
}
