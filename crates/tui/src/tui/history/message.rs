//! User, assistant, and system message transcript rendering.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tui::markdown_render;
use crate::tui::ui_text::CopyLineSeparator;

use super::{ASSISTANT_GLYPH, USER_GLYPH};

pub(crate) struct RenderedTranscriptLine {
    pub line: Line<'static>,
    pub copy_prefix_width: usize,
    pub copy_separator_after: CopyLineSeparator,
}

pub(super) fn render_message(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    render_message_with_copy_metadata(prefix, label_style, body_style, content, width)
        .into_iter()
        .map(|rendered| rendered.line)
        .collect()
}

pub(super) fn render_message_with_copy_metadata(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<RenderedTranscriptLine> {
    // An assistant cell whose content is entirely whitespace (e.g. a stray
    // newline streamed between reasoning and a tool call) would otherwise
    // render as a bare, orphaned role glyph floating on its own line — the
    // "blue dots with nothing after them" artifact. Render nothing so the
    // transcript doesn't accumulate empty markers. Real prose, including
    // messages that merely start with blank lines, still renders normally.
    if prefix == ASSISTANT_GLYPH && content.trim().is_empty() {
        return Vec::new();
    }
    let prefix_width = UnicodeWidthStr::width(prefix);
    let prefix_width_u16 = u16::try_from(prefix_width.saturating_add(2)).unwrap_or(u16::MAX);
    let content_width = usize::from(width.saturating_sub(prefix_width_u16).max(1));
    let mut lines = Vec::new();
    let rendered =
        markdown_render::render_markdown_tagged(content, content_width as u16, body_style);
    for (idx, rendered_line) in rendered.into_iter().enumerate() {
        let line = if idx == 0 {
            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix.to_string(),
                    label_style.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(rendered_line.line.spans);
            Line::from(spans)
        } else {
            let indent = if prefix.is_empty() {
                String::new()
            } else if rendered_line.is_code {
                " ".repeat(prefix_width + 1)
            } else {
                let mut s = String::with_capacity(prefix_width + 1);
                s.push('\u{258F}');
                s.extend(std::iter::repeat_n(' ', prefix_width));
                s
            };
            let rail_style = Style::default().fg(palette::TEXT_DIM);
            let mut spans = vec![Span::styled(indent, rail_style)];
            spans.extend(rendered_line.line.spans);
            Line::from(spans)
        };
        lines.push(RenderedTranscriptLine {
            line,
            copy_prefix_width: rendered_line.copy_prefix_width
                + history_copy_prefix_width(prefix, prefix_width, rendered_line.is_code, idx),
            copy_separator_after: rendered_line.copy_separator_after,
        });
    }
    if lines.is_empty() {
        lines.push(RenderedTranscriptLine {
            line: Line::from(""),
            copy_prefix_width: 0,
            copy_separator_after: CopyLineSeparator::Newline,
        });
    }
    lines
}

fn history_copy_prefix_width(
    prefix: &str,
    prefix_width: usize,
    is_code: bool,
    line_index: usize,
) -> usize {
    if line_index > 0 && is_code && !prefix.is_empty() {
        prefix_width + 1
    } else {
        0
    }
}

pub(super) fn hard_break_copy_lines(lines: Vec<Line<'static>>) -> Vec<RenderedTranscriptLine> {
    lines
        .into_iter()
        .map(|line| RenderedTranscriptLine {
            line,
            copy_prefix_width: 0,
            copy_separator_after: CopyLineSeparator::Newline,
        })
        .collect()
}

/// Render a plain-text user message: split on newlines, word-wrap each line,
/// preserve leading whitespace. No markdown interpretation (headings, lists,
/// code blocks, etc. are rendered as literal text).
pub(super) fn render_plain_message(
    prefix: &str,
    label_style: Style,
    body_style: Style,
    content: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let prefix_width = UnicodeWidthStr::width(prefix);
    let prefix_width_u16 = u16::try_from(prefix_width.saturating_add(2)).unwrap_or(u16::MAX);
    let content_width = width.saturating_sub(prefix_width_u16).max(1);
    let rendered = markdown_render::render_plain_text(content, content_width, body_style);
    let mut lines = Vec::with_capacity(rendered.len());

    for (idx, line) in rendered.into_iter().enumerate() {
        if idx == 0 {
            let mut spans = Vec::new();
            if !prefix.is_empty() {
                spans.push(Span::styled(
                    prefix.to_string(),
                    label_style.add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::raw(" "));
            }
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        } else {
            let indent = if prefix.is_empty() {
                String::new()
            } else {
                let mut s = String::with_capacity(prefix_width + 1);
                s.push('\u{258F}');
                s.extend(std::iter::repeat_n(' ', prefix_width));
                s
            };
            let rail_style = Style::default().fg(palette::TEXT_DIM);
            let mut spans = vec![Span::styled(indent, rail_style)];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

pub(super) fn render_user_message(content: &str, width: u16) -> Vec<Line<'static>> {
    render_plain_message(
        USER_GLYPH,
        user_label_style(),
        user_body_style(),
        content,
        width,
    )
    .into_iter()
    .map(|line| apply_user_message_highlight(line, width))
    .collect()
}

fn apply_user_message_highlight(mut line: Line<'static>, width: u16) -> Line<'static> {
    let bg = palette::SURFACE_ELEVATED;
    line.style = line.style.bg(bg);

    let target_width = usize::from(width);
    let line_width = line.width();
    if line_width < target_width {
        line.spans.push(Span::styled(
            " ".repeat(target_width - line_width),
            Style::default().bg(bg),
        ));
    }

    line
}

pub(super) fn user_label_style() -> Style {
    Style::default().fg(palette::USER_BODY)
}

pub(super) fn user_body_style() -> Style {
    Style::default().fg(palette::USER_BODY)
}

/// Style for the assistant glyph (`●`). When the cell is streaming and
/// motion is allowed, the foreground pulses on a 2s cycle between 30% and
/// 100% brightness — the only deliberately animated element in a calm
/// transcript. When idle (or low_motion is on) it sits at the full DeepSeek
/// sky color so finished turns read as solid rather than dim.
pub(super) fn assistant_label_style_for(streaming: bool, low_motion: bool) -> Style {
    let color = if streaming && !low_motion {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        palette::pulse_brightness(palette::WHALE_INFO, now_ms)
    } else {
        palette::WHALE_INFO
    };
    Style::default().fg(color)
}

pub(super) fn system_label_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

pub(super) fn message_body_style() -> Style {
    Style::default().fg(palette::TEXT_PRIMARY)
}

pub(super) fn system_body_style() -> Style {
    Style::default().fg(palette::TEXT_MUTED).italic()
}
