//! Shared text helpers for TUI selection and clipboard workflows.

use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::history::HistoryCell;
use crate::tui::osc8;

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

pub(crate) fn concise_shell_command_label(command: &str, max_width: usize) -> String {
    let normalized = normalize_shell_text(command);
    if let Some(label) = gh_command_label(&normalized) {
        return truncate_line_to_width(&label, max_width);
    }

    let segment = actionable_shell_segment(&normalized).unwrap_or_else(|| normalized.clone());
    truncate_line_to_width(&segment, max_width)
}

fn normalize_shell_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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

    #[test]
    fn concise_shell_command_label_prefers_gh_pr_checks_over_wrappers() {
        let label = concise_shell_command_label(
            "cd /tmp/repo && sleep 15 && gh pr checks 1611 --repo Hmbown/DeepSeek-TUI",
            80,
        );
        assert_eq!(label, "gh pr checks 1611");
    }

    #[test]
    fn concise_shell_command_label_falls_back_to_actionable_segment() {
        let label = concise_shell_command_label("cd /tmp/repo && cargo test --workspace", 80);
        assert_eq!(label, "cargo test --workspace");
    }
}
