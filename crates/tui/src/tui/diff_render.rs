//! Diff rendering helpers for TUI previews.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;

const LINE_NUMBER_WIDTH: usize = 4;

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

    if !summaries.is_empty() {
        lines.extend(render_diff_summary(&summaries, width));
    }

    for raw in diff.lines() {
        if raw.starts_with("diff --git") || raw.starts_with("index ") {
            lines.extend(render_header_line(raw, width));
            continue;
        }

        if raw.starts_with("--- ") || raw.starts_with("+++ ") {
            lines.extend(render_header_line(raw, width));
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = old_start;
                new_line = new_start;
            }
            lines.extend(render_hunk_header(raw, width));
            continue;
        }

        if raw.starts_with('+') && !raw.starts_with("+++") {
            let content = raw.strip_prefix('+').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '+',
                Style::default()
                    .fg(palette::DIFF_ADDED)
                    .bg(palette::DIFF_ADDED_BG),
            ));
            if let Some(line) = new_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if raw.starts_with('-') && !raw.starts_with("---") {
            let content = raw.strip_prefix('-').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '-',
                Style::default()
                    .fg(palette::STATUS_ERROR)
                    .bg(palette::DIFF_DELETED_BG),
            ));
            if let Some(line) = old_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if raw.starts_with(' ') {
            let content = raw.strip_prefix(' ').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                ' ',
                Style::default().fg(palette::TEXT_PRIMARY),
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

/// Render a unified diff in the **compact** layout used by the approval
/// popup: keeps `@@` hunk headers, line numbers, and `+/-` colouring, but
/// drops the multi-line summary block and the `--- a/... +++ b/...` /
/// `diff --git` headers. That trim is what frees up the ~6 rows the old
/// renderer was burning on metadata inside a 10-line preview window.
///
/// The full renderer (`render_diff`) is still what the detail pager uses,
/// so nothing here changes what the user sees behind `v`.
pub fn render_diff_compact(diff: &str, width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut old_line: Option<usize> = None;
    let mut new_line: Option<usize> = None;

    for raw in diff.lines() {
        if raw.starts_with("diff --git")
            || raw.starts_with("index ")
            || raw.starts_with("--- ")
            || raw.starts_with("+++ ")
        {
            continue;
        }

        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = old_start;
                new_line = new_start;
            }
            lines.extend(render_hunk_header(raw, width));
            continue;
        }

        if raw.starts_with('+') {
            let content = raw.strip_prefix('+').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '+',
                Style::default()
                    .fg(palette::DIFF_ADDED)
                    .bg(palette::DIFF_ADDED_BG),
            ));
            if let Some(line) = new_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if raw.starts_with('-') {
            let content = raw.strip_prefix('-').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                '-',
                Style::default()
                    .fg(palette::STATUS_ERROR)
                    .bg(palette::DIFF_DELETED_BG),
            ));
            if let Some(line) = old_line.as_mut() {
                *line = line.saturating_add(1);
            }
            continue;
        }

        if raw.starts_with(' ') {
            let content = raw.strip_prefix(' ').unwrap_or(raw);
            lines.extend(render_diff_line(
                content,
                width,
                old_line,
                new_line,
                ' ',
                Style::default().fg(palette::TEXT_PRIMARY),
            ));
            if let Some(line) = old_line.as_mut() {
                *line = line.saturating_add(1);
            }
            if let Some(line) = new_line.as_mut() {
                *line = line.saturating_add(1);
            }
        }
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

fn render_diff_summary(summaries: &[DiffFileSummary], width: u16) -> Vec<Line<'static>> {
    let files = summaries.len();
    let added: usize = summaries.iter().map(|summary| summary.added).sum();
    let deleted: usize = summaries.iter().map(|summary| summary.deleted).sum();
    let hunks: usize = summaries.iter().map(|summary| summary.hunks).sum();

    let mut lines = Vec::new();
    lines.extend(wrap_with_style(
        &format!(
            "summary: {files} file{}, +{added} -{deleted}, {hunks} hunk{}",
            if files == 1 { "" } else { "s" },
            if hunks == 1 { "" } else { "s" },
        ),
        Style::default()
            .fg(palette::TEXT_PRIMARY)
            .add_modifier(Modifier::BOLD),
        width,
    ));
    for summary in summaries {
        let row = format!(
            "  {}  +{} -{}  {} hunk{}",
            summary.path,
            summary.added,
            summary.deleted,
            summary.hunks,
            if summary.hunks == 1 { "" } else { "s" },
        );
        lines.extend(wrap_with_style(
            &row,
            Style::default().fg(palette::TEXT_MUTED),
            width,
        ));
    }
    lines
}

fn parse_hunk_header(line: &str) -> Option<(Option<usize>, Option<usize>)> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 3 {
        return None;
    }
    let old = parts[1].trim_start_matches('-');
    let new = parts[2].trim_start_matches('+');
    // 0 means "no lines" (e.g. new file) → None so the gutter stays blank.
    let old_start = old
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|&v| v > 0);
    let new_start = new
        .split(',')
        .next()?
        .parse::<usize>()
        .ok()
        .filter(|&v| v > 0);
    Some((old_start, new_start))
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
    style: Style,
) -> Vec<Line<'static>> {
    let prefix = format_line_numbers(old_line, new_line, marker);
    let prefix_width = prefix.width();
    let available = width.saturating_sub(prefix_width as u16).max(1) as usize;
    let wrapped = wrap_text(content, available);

    let mut out = Vec::new();
    for (idx, chunk) in wrapped.into_iter().enumerate() {
        if idx == 0 {
            out.push(Line::from(vec![
                Span::styled(prefix.clone(), Style::default().fg(palette::TEXT_MUTED)),
                Span::styled(chunk, style),
            ]));
        } else {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(prefix_width)),
                Span::styled(chunk, style),
            ]));
        }
    }

    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(
            prefix,
            Style::default().fg(palette::TEXT_MUTED),
        )]));
    }

    out
}

fn format_line_numbers(old_line: Option<usize>, new_line: Option<usize>, marker: char) -> String {
    let old = old_line
        .map(|value| format!("{value:>LINE_NUMBER_WIDTH$}"))
        .unwrap_or_else(|| " ".repeat(LINE_NUMBER_WIDTH));
    let new = new_line
        .map(|value| format!("{value:>LINE_NUMBER_WIDTH$}"))
        .unwrap_or_else(|| " ".repeat(LINE_NUMBER_WIDTH));
    format!("{old} {new} {marker} ")
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

    for ch in text.chars() {
        let char_width = ch.width().unwrap_or(1);
        if current_width + char_width > width && current_width > 0 {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += char_width;
    }

    if current.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(current);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(text[0].contains("summary: 1 file, +1 -1, 1 hunk"));
        assert!(text.iter().any(|line| line.contains("src/a.rs")));
        assert!(text.iter().any(|line| line.contains("+1 -1")));
        assert!(
            text.iter().any(|line| line.contains(" + new")),
            "added line should carry + gutter: {text:?}"
        );
        assert!(
            text.iter().any(|line| line.contains(" - old")),
            "deleted line should carry - gutter: {text:?}"
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

    #[test]
    fn render_diff_compact_skips_summary_and_file_headers() {
        // The compact renderer is what the approval popup uses, so summary
        // / `diff --git` / `--- +++` rows must be dropped to free vertical
        // room for actual code. Hunk header + line-numbered code rows stay.
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 line
+new
-old
";

        let rendered = render_diff_compact(diff, 80);
        let text: Vec<String> = rendered.iter().map(line_text).collect();
        let joined = text.join("\n");
        assert!(
            !joined.contains("summary:"),
            "compact must skip summary: {joined}"
        );
        assert!(
            !joined.contains("diff --git"),
            "compact must skip file headers: {joined}",
        );
        assert!(
            !joined.contains("--- a/src/a.rs"),
            "compact must skip --- header: {joined}",
        );
        assert!(
            text.iter().any(|line| line.contains("@@ -1,2 +1,3 @@")),
            "hunk header should remain: {text:?}",
        );
        assert!(
            text.iter().any(|line| line.contains(" + new")),
            "added line should carry + gutter: {text:?}",
        );
        assert!(
            text.iter().any(|line| line.contains(" - old")),
            "deleted line should carry - gutter: {text:?}",
        );
    }

    #[test]
    fn render_diff_preserves_indented_context_lines() {
        let diff = "\
diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,2 @@
 fn main() {
     println!(\"hello\");
";
        let rendered = render_diff_compact(diff, 80);
        let text = rendered
            .iter()
            .map(line_text)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains(" fn main() {"), "{text}");
        assert!(text.contains("     println!"), "{text}");
    }
}
