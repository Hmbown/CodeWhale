//! Diff rendering helpers for TUI previews.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;

const MIN_LINE_NUMBER_WIDTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFileSummary {
    pub path: String,
    pub added: usize,
    pub deleted: usize,
    pub hunks: usize,
}

pub fn render_diff(diff: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut old_line: Option<usize> = None;
    let mut new_line: Option<usize> = None;
    let summaries = summarize_diff(diff);
    let line_number_width = diff_line_number_width(diff);
    let show_file_headers = summaries.len() > 1;

    if !summaries.is_empty() {
        lines.extend(render_diff_summary(&summaries, width));
    }

    let mut current_file: Option<String> = None;
    for raw in diff.lines() {
        if raw.starts_with("diff --git ") {
            let path = parse_diff_git_path(raw).unwrap_or_else(|| "<file>".to_string());
            if show_file_headers && current_file.as_deref() != Some(&path) {
                current_file = Some(path.clone());
                lines.extend(render_file_header(&path, width));
            }
            continue;
        }
        if raw.starts_with("index ") {
            continue;
        }
        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = Some(old_start);
                new_line = Some(new_start);
            }
            lines.extend(render_hunk_header(raw, width));
            continue;
        }

        if let Some(content) = raw.strip_prefix('+') {
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '+',
                Style::default()
                    .fg(palette::DIFF_ADDED)
                    .bg(palette::DIFF_ADDED_BG),
                Style::default().fg(Color::White).bg(palette::DIFF_ADDED_BG),
                true,
                line_number_width,
            ));
            if let Some(line) = new_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if let Some(content) = raw.strip_prefix('-') {
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '-',
                Style::default()
                    .fg(palette::DIFF_DELETED)
                    .bg(palette::DIFF_DELETED_BG),
                Style::default()
                    .fg(Color::White)
                    .bg(palette::DIFF_DELETED_BG),
                true,
                line_number_width,
            ));
            if let Some(line) = old_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if let Some(content) = raw.strip_prefix(' ') {
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                ' ',
                Style::default().fg(palette::TEXT_TOOL_SUMMARY_DIM),
                Style::default().fg(Color::White),
                false,
                line_number_width,
            ));
            if let Some(line) = old_line.as_mut() {
                *line = line.saturating_add(1);
            }
            if let Some(line) = new_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        lines.extend(render_header_line(raw, width));
    }

    lines
}

#[must_use]
pub fn summarize_diff(diff: &str) -> Vec<DiffFileSummary> {
    let mut summaries = Vec::new();
    let mut current: Option<DiffFileSummary> = None;

    for raw in diff.lines() {
        if raw.starts_with("diff --git ") {
            if let Some(summary) = current.take()
                && summary.has_changes()
            {
                summaries.push(summary);
            }
            current = Some(DiffFileSummary {
                path: parse_diff_git_path(raw).unwrap_or_else(|| "<file>".to_string()),
                added: 0,
                deleted: 0,
                hunks: 0,
            });
            continue;
        }

        if raw.starts_with("+++ ") {
            let path = raw
                .trim_start_matches("+++ ")
                .trim_start_matches("b/")
                .to_string();
            if path != "/dev/null" {
                current
                    .get_or_insert_with(|| DiffFileSummary {
                        path: path.clone(),
                        added: 0,
                        deleted: 0,
                        hunks: 0,
                    })
                    .path = path.clone();
            }
            continue;
        }

        if raw.starts_with("@@") {
            current
                .get_or_insert_with(|| DiffFileSummary {
                    path: "<file>".to_string(),
                    added: 0,
                    deleted: 0,
                    hunks: 0,
                })
                .hunks += 1;
            continue;
        }

        if raw.starts_with('+') && !raw.starts_with("+++") {
            current
                .get_or_insert_with(|| DiffFileSummary {
                    path: "<file>".to_string(),
                    added: 0,
                    deleted: 0,
                    hunks: 0,
                })
                .added += 1;
        } else if raw.starts_with('-') && !raw.starts_with("---") {
            current
                .get_or_insert_with(|| DiffFileSummary {
                    path: "<file>".to_string(),
                    added: 0,
                    deleted: 0,
                    hunks: 0,
                })
                .deleted += 1;
        }
    }

    if let Some(summary) = current
        && summary.has_changes()
    {
        summaries.push(summary);
    }

    summaries
}

#[must_use]
pub fn diff_summary_label(diff: &str) -> Option<String> {
    let summaries = summarize_diff(diff);
    if summaries.is_empty() {
        return None;
    }
    let files = summaries.len();
    let added: usize = summaries.iter().map(|summary| summary.added).sum();
    let deleted: usize = summaries.iter().map(|summary| summary.deleted).sum();
    Some(format!(
        "{files} file{} +{added} -{deleted}",
        if files == 1 { "" } else { "s" }
    ))
}

impl DiffFileSummary {
    fn has_changes(&self) -> bool {
        self.added > 0 || self.deleted > 0 || self.hunks > 0
    }
}

fn parse_diff_git_path(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    let _diff = parts.next()?;
    let _git = parts.next()?;
    let _old = parts.next()?;
    let new = parts.next()?;
    Some(new.trim_start_matches("b/").to_string())
}

fn render_file_header(path: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    let bar = "\u{2500}".repeat(width.saturating_sub(path.width() as u16 + 4).max(1) as usize);
    lines.push(Line::from(vec![
        Span::styled("── ", Style::default().fg(palette::TEXT_DIM)),
        Span::styled(
            path.to_string(),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {bar}"), Style::default().fg(palette::TEXT_DIM)),
    ]));
    lines
}

fn render_diff_summary(summaries: &[DiffFileSummary], width: u16) -> Vec<Line<'static>> {
    let added: usize = summaries.iter().map(|summary| summary.added).sum();
    let deleted: usize = summaries.iter().map(|summary| summary.deleted).sum();

    let mut lines = Vec::new();
    lines.extend(wrap_with_style(
        &format!(
            "Added {added} line{}, removed {deleted} line{}",
            if added == 1 { "" } else { "s" },
            if deleted == 1 { "" } else { "s" },
        ),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
        width,
    ));
    lines
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let old = parts[1].trim_start_matches('-');
    let new = parts[2].trim_start_matches('+');
    let old_start = old.split(',').next()?.parse::<usize>().ok()?;
    let new_start = new.split(',').next()?.parse::<usize>().ok()?;
    Some((old_start, new_start))
}

fn diff_line_number_width(diff: &str) -> usize {
    let mut max_line = 0usize;
    for raw in diff.lines().filter(|line| line.starts_with("@@")) {
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        if let Some((start, count)) = parse_hunk_range(parts[1].trim_start_matches('-')) {
            max_line = max_line.max(start.saturating_add(count.saturating_sub(1)));
        }
        if let Some((start, count)) = parse_hunk_range(parts[2].trim_start_matches('+')) {
            max_line = max_line.max(start.saturating_add(count.saturating_sub(1)));
        }
    }
    max_line.to_string().len().max(MIN_LINE_NUMBER_WIDTH)
}

fn parse_hunk_range(range: &str) -> Option<(usize, usize)> {
    let mut parts = range.split(',');
    let start = parts.next()?.parse::<usize>().ok()?;
    let count = parts
        .next()
        .map(|value| value.parse::<usize>().ok())
        .unwrap_or(Some(1))?;
    Some((start, count))
}

fn render_header_line(line: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(palette::DEEPSEEK_SKY)
        .add_modifier(Modifier::BOLD);
    wrap_with_style(line, style, width)
}

fn render_hunk_header(line: &str, width: u16) -> Vec<Line<'static>> {
    let style = Style::default().fg(palette::DEEPSEEK_BLUE);
    wrap_with_style(line, style, width)
}

fn render_diff_line(
    content: &str,
    width: u16,
    old_line: Option<usize>,
    new_line: Option<usize>,
    marker: char,
    prefix_style: Style,
    content_style: Style,
    fill_to_width: bool,
    line_number_width: usize,
) -> Vec<Line<'static>> {
    let prefix = format_line_number(old_line, new_line, marker, line_number_width);
    let continuation_prefix = format_line_number(None, None, marker, line_number_width);
    let prefix_width = prefix.width();
    let available = width.saturating_sub(prefix_width as u16).max(1) as usize;
    let wrapped = wrap_code_text(content, available);

    let mut out = Vec::new();
    for (idx, chunk) in wrapped.into_iter().enumerate() {
        let chunk = if fill_to_width {
            pad_to_width(&chunk, available)
        } else {
            chunk
        };
        if idx == 0 {
            out.push(Line::from(vec![
                Span::styled(prefix.clone(), prefix_style),
                Span::styled(chunk, content_style),
            ]));
        } else {
            out.push(Line::from(vec![
                Span::styled(continuation_prefix.clone(), prefix_style),
                Span::styled(chunk, content_style),
            ]));
        }
    }

    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(prefix, prefix_style)]));
    }

    out
}

fn format_line_number(
    old_line: Option<usize>,
    new_line: Option<usize>,
    marker: char,
    line_number_width: usize,
) -> String {
    let line = match marker {
        '+' => new_line,
        '-' => old_line,
        _ => new_line.or(old_line),
    };
    let number = line
        .map(|value| format!("{value:>line_number_width$}"))
        .unwrap_or_else(|| " ".repeat(line_number_width));
    format!("{number} {marker} ")
}

fn wrap_with_style(text: &str, style: Style, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for part in wrap_text(text, width.max(1) as usize) {
        out.push(Line::from(Span::styled(part, style)));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled("", style)));
    }
    out
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for word in text.split_whitespace() {
        let word_width = word.width();
        if word_width > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            push_word_breaking_chars(word, width, &mut current, &mut current_width, &mut lines);
            continue;
        }
        let additional = if current.is_empty() {
            word_width
        } else {
            word_width + 1
        };
        if current_width + additional > width && !current.is_empty() {
            lines.push(current);
            current = word.to_string();
            current_width = word_width;
        } else {
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

fn wrap_code_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

fn pad_to_width(text: &str, width: usize) -> String {
    let current = text.width();
    if current >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - current))
    }
}

fn push_word_breaking_chars(
    word: &str,
    width: usize,
    current: &mut String,
    current_width: &mut usize,
    lines: &mut Vec<String>,
) {
    for ch in word.chars() {
        let char_width = ch.width().unwrap_or(1);
        if *current_width + char_width > width && *current_width > 0 {
            lines.push(std::mem::take(current));
            *current_width = 0;
        }
        current.push(ch);
        *current_width += char_width;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn summarizes_multi_file_diff() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 line
+new
-old
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -10,0 +11,2 @@
+one
+two
";

        let summaries = summarize_diff(diff);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].path, "src/a.rs");
        assert_eq!(summaries[0].added, 1);
        assert_eq!(summaries[0].deleted, 1);
        assert_eq!(summaries[1].path, "src/b.rs");
        assert_eq!(summaries[1].added, 2);
        assert_eq!(summaries[1].deleted, 0);
        assert_eq!(diff_summary_label(diff).as_deref(), Some("2 files +3 -1"));
    }

    #[test]
    fn render_diff_prepends_summary_and_gutter_markers() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 line
+new
-old
";

        let rendered = render_diff(diff, 80);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();
        assert!(text[0].contains("Added 1 line, removed 1 line"));
        assert!(
            !text.iter().any(|line| line.contains("── src/a.rs")),
            "single-file diff should not add an extra file divider: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("   2 + new")),
            "added line should carry + gutter: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains("   2 - old")),
            "deleted line should carry - gutter: {text:?}"
        );

        let added = rendered
            .iter()
            .find(|line| line_text(line).contains("   2 + new"))
            .expect("added line");
        assert_eq!(added.spans[0].style.bg, Some(palette::DIFF_ADDED_BG));
        assert_eq!(added.spans[0].style.fg, Some(palette::DIFF_ADDED));
        assert_eq!(added.spans[1].style.bg, Some(palette::DIFF_ADDED_BG));
        assert_eq!(added.spans[1].style.fg, Some(Color::White));

        let deleted = rendered
            .iter()
            .find(|line| line_text(line).contains("   2 - old"))
            .expect("deleted line");
        assert_eq!(deleted.spans[0].style.bg, Some(palette::DIFF_DELETED_BG));
        assert_eq!(deleted.spans[0].style.fg, Some(palette::DIFF_DELETED));
        assert_eq!(deleted.spans[1].style.bg, Some(palette::DIFF_DELETED_BG));
        assert_eq!(deleted.spans[1].style.fg, Some(Color::White));
    }

    #[test]
    fn render_diff_preserves_code_indent_and_prefix_operators() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
-    --counter;
+    ++counter;
 context
";

        let rendered = render_diff(diff, 80);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();
        assert!(
            text.iter()
                .any(|line| line.contains("   1 -     --counter;")),
            "deleted code indentation/operator prefix should survive: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.contains("   1 +     ++counter;")),
            "added code indentation/operator prefix should survive: {text:?}"
        );
    }

    #[test]
    fn render_diff_uses_single_dynamic_line_number_gutter() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -15350,2 +15351,2 @@
-old value that is long enough to wrap when the viewport is narrow
+new value that is long enough to wrap when the viewport is narrow
";

        let rendered = render_diff(diff, 48);
        let text = rendered.iter().map(line_text).collect::<Vec<_>>();

        assert!(
            text.iter()
                .any(|line| line.starts_with("15350 - old value")),
            "deleted line should use one old line-number column: {text:?}"
        );
        assert!(
            text.iter()
                .any(|line| line.starts_with("15351 + new value")),
            "added line should use one new line-number column: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.starts_with("      - ")),
            "wrapped deleted continuation should repeat the marker with blank number: {text:?}"
        );
        assert!(
            text.iter().all(|line| !line.starts_with("15350 15351")),
            "old/new double gutter must not render: {text:?}"
        );
    }

    #[test]
    fn wrap_text_breaks_overlong_cjk_runs() {
        let text = "这是一个非常长的中文字符串".repeat(10);
        let lines = wrap_text(&text, 16);

        for line in &lines {
            assert!(line.width() <= 16, "line {line:?} exceeds width 16");
        }

        assert_eq!(lines.join(""), text);
    }
}
