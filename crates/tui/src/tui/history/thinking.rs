//! Rendering for reasoning/thinking transcript cells.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;
use crate::tui::markdown_render;

/// Reasoning header opener. Replaces the spinner glyph on thinking cells —
/// reasoning is a slow exhale, not a tool spin.
pub(super) const REASONING_OPENER: &str = "\u{2026}"; // …
/// Reasoning body left rail. Dashed (`╎`) instead of the solid `▏` block to
/// visually separate reasoning from message body and tool output.
pub(super) const REASONING_RAIL: &str = "\u{254E} "; // ╎ + space
/// Trailing-line cursor on streaming reasoning. Anchored to the live colour
/// so the user sees where new tokens land.
pub(super) const REASONING_CURSOR: &str = "\u{258E}"; // ▎

const THINKING_SUMMARY_LINE_LIMIT: usize = 4;
/// Completed collapsed thought: a short lede, not a ten-line dump.
/// Grok's finished thought is header-only; we keep two lines so a one-step
/// thought is still readable without forcing an expand.
const THINKING_COMPLETED_PREVIEW_LINE_LIMIT: usize = 2;
const THINKING_STREAMING_PREVIEW_LINE_LIMIT: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingVisualState {
    Live,
    Done,
    Idle,
}

#[cfg(test)]
#[must_use]
pub fn extract_reasoning_summary(text: &str) -> Option<String> {
    extract_explicit_reasoning_summary(text).or_else(|| {
        let fallback = text.trim();
        if fallback.is_empty() {
            None
        } else {
            Some(fallback.to_string())
        }
    })
}

fn extract_explicit_reasoning_summary(text: &str) -> Option<String> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.to_lowercase().starts_with("summary") {
            let mut summary = String::new();
            if let Some((_, rest)) = trimmed.split_once(':')
                && !rest.trim().is_empty()
            {
                summary.push_str(rest.trim());
                summary.push('\n');
            }
            while let Some(next) = lines.peek() {
                let next_trimmed = next.trim();
                if next_trimmed.is_empty() {
                    break;
                }
                if next_trimmed.starts_with('#') || next_trimmed.starts_with("**") {
                    break;
                }
                summary.push_str(next_trimmed);
                summary.push('\n');
                lines.next();
            }
            let summary = summary.trim().to_string();
            return if summary.is_empty() {
                None
            } else {
                Some(summary)
            };
        }
    }
    None
}

pub(super) fn render_thinking(
    content: &str,
    width: u16,
    streaming: bool,
    duration_secs: Option<f32>,
    collapsed: bool,
    low_motion: bool,
) -> Vec<Line<'static>> {
    render_thinking_with_analysis(
        content,
        width,
        streaming,
        duration_secs,
        collapsed,
        low_motion,
        true,
    )
    .0
}

pub(crate) fn render_thinking_with_analysis(
    content: &str,
    width: u16,
    streaming: bool,
    duration_secs: Option<f32>,
    collapsed: bool,
    low_motion: bool,
    highlight: bool,
) -> (Vec<Line<'static>>, bool) {
    render_thinking_with_preview_limit(
        content,
        width,
        streaming,
        duration_secs,
        collapsed,
        low_motion,
        highlight,
        0,
        THINKING_COMPLETED_PREVIEW_LINE_LIMIT,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_thinking_with_preview_limit(
    content: &str,
    width: u16,
    streaming: bool,
    duration_secs: Option<f32>,
    collapsed: bool,
    low_motion: bool,
    highlight: bool,
    preview_extra_lines: usize,
    completed_preview_lines: usize,
) -> (Vec<Line<'static>>, bool) {
    let state = thinking_visual_state(streaming, duration_secs);
    let style = thinking_style();
    // 12% reasoning surface tint over the app ink — the only deliberately
    // warm element in the transcript. Dropped on Ansi-16 terminals where the
    // tint would distort the named palette.
    let depth = cached_color_depth();
    let body_bg = palette::reasoning_surface_tint(depth);
    let body_style = match (highlight, body_bg) {
        (true, Some(bg)) => style.italic().bg(bg),
        (_, None) | (false, Some(_)) => style.italic(),
    };
    let mut lines = Vec::new();

    // Header: `…` opener (replaces the spinner; reasoning isn't a tool, it's
    // a slow exhale) followed by the reasoning label and live status.
    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        Span::styled("reasoning", thinking_title_style()),
    ];
    header_spans.push(Span::styled(" ", Style::default()));
    header_spans.push(Span::styled(
        thinking_status_label(state),
        thinking_status_style(state),
    ));
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(
            crate::elapsed::format_elapsed_ms((dur * 1000.0) as u64),
            thinking_meta_style(),
        ));
    }
    lines.push(Line::from(header_spans));

    let content_width = width.saturating_sub(3).max(1);
    let (collapsed_body, expandable) = collapsed_thinking_body(
        content,
        width,
        streaming,
        body_style,
        preview_extra_lines,
        completed_preview_lines,
    );
    let rendered = if collapsed {
        collapsed_body
    } else if content.trim().is_empty() {
        Vec::new()
    } else {
        markdown_render::render_markdown(content, content_width, body_style)
    };

    let rail_style = Style::default().fg(thinking_state_accent(state));
    let cursor_style = Style::default().fg(palette::ACCENT_REASONING_LIVE);

    if rendered.is_empty() && streaming {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.push(Span::styled("reasoning...", body_style.italic()));
        if !low_motion {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let last_idx = rendered.len().saturating_sub(1);
    for (idx, line) in rendered.into_iter().enumerate() {
        let mut spans = vec![Span::styled(REASONING_RAIL.to_string(), rail_style)];
        spans.extend(line.spans);
        // Mark only the live tail; styling every line would churn the block.
        if streaming && !low_motion && idx == last_idx {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    if collapsed && expandable {
        lines.push(Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(
                REASONING_OPENER,
                Style::default().fg(palette::TEXT_MUTED).italic(),
            ),
        ]));
    }

    (lines, expandable)
}

fn collapsed_thinking_body(
    content: &str,
    width: u16,
    streaming: bool,
    style: Style,
    preview_extra_lines: usize,
    completed_preview_lines: usize,
) -> (Vec<Line<'static>>, bool) {
    let (body_text, without_explicit_summary) = if streaming {
        // #861 RC4 / #1324: an in-flight block has no meaningful completed
        // summary. Render raw content; the limit below keeps its newest lines.
        (content.to_string(), false)
    } else {
        match extract_explicit_reasoning_summary(content) {
            Some(summary) => (summary, false),
            None => (content.to_string(), true),
        }
    };
    // #4146/#4148 used to scrub snake_case here. That rule could not tell
    // CodeWhale identifiers from user identifiers: paths, env vars, and
    // module names became bare ellipses while the full body remained one
    // keypress away. Keep the default view readable; do not revive the scrub.
    let mut lines = if body_text.trim().is_empty() {
        Vec::new()
    } else {
        markdown_render::render_markdown(&body_text, width.saturating_sub(3).max(1), style)
    };
    let limit = if streaming {
        THINKING_STREAMING_PREVIEW_LINE_LIMIT.saturating_add(preview_extra_lines)
    } else if without_explicit_summary {
        completed_preview_lines.saturating_add(preview_extra_lines)
    } else {
        THINKING_SUMMARY_LINE_LIMIT
    };
    let truncated = lines.len() > limit;
    if truncated {
        if streaming {
            // Follow the live cursor: discard the head, not the newest lines.
            lines.drain(0..lines.len() - limit);
        } else {
            lines.truncate(limit);
        }
    }
    let meaningful = truncated || (!streaming && body_text.trim() != content.trim());
    (lines, meaningful)
}

pub(super) fn render_hidden_thinking_activity(
    _width: u16,
    duration_secs: Option<f32>,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let state = ThinkingVisualState::Live;
    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        // A hidden live block needs one receipt, not stacked variants of the
        // same state ("reasoning live" plus "reasoning hidden; working").
        Span::styled("reasoning hidden", thinking_title_style()),
    ];
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(
            crate::elapsed::format_elapsed_ms((dur * 1000.0) as u64),
            thinking_meta_style(),
        ));
    }
    if !low_motion {
        header_spans.push(Span::styled(
            format!(" {REASONING_CURSOR}"),
            Style::default().fg(palette::ACCENT_REASONING_LIVE),
        ));
    }
    vec![Line::from(header_spans)]
}

fn thinking_style() -> Style {
    Style::default().fg(palette::TEXT_REASONING)
}

fn thinking_visual_state(streaming: bool, duration_secs: Option<f32>) -> ThinkingVisualState {
    if streaming {
        ThinkingVisualState::Live
    } else if duration_secs.is_some() {
        ThinkingVisualState::Done
    } else {
        ThinkingVisualState::Idle
    }
}

fn thinking_status_label(state: ThinkingVisualState) -> &'static str {
    match state {
        ThinkingVisualState::Live => "live",
        ThinkingVisualState::Done => "done",
        ThinkingVisualState::Idle => "idle",
    }
}

fn thinking_title_style() -> Style {
    Style::default()
        .fg(palette::TEXT_SOFT)
        .add_modifier(Modifier::BOLD)
}

fn thinking_status_style(state: ThinkingVisualState) -> Style {
    Style::default().fg(match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    })
}

fn thinking_meta_style() -> Style {
    Style::default().fg(palette::TEXT_DIM)
}

fn thinking_state_accent(state: ThinkingVisualState) -> Color {
    match state {
        ThinkingVisualState::Live => palette::ACCENT_REASONING_LIVE,
        ThinkingVisualState::Done => palette::TEXT_DIM,
        ThinkingVisualState::Idle => palette::TEXT_DIM,
    }
}

/// Once-initialised colour depth for the terminal session. Avoids re-reading
/// `COLORTERM` / `TERM` env vars on every frame.
static COLOR_DEPTH: std::sync::OnceLock<palette::ColorDepth> = std::sync::OnceLock::new();

pub(super) fn cached_color_depth() -> palette::ColorDepth {
    *COLOR_DEPTH.get_or_init(palette::ColorDepth::detect)
}
