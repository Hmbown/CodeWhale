//! Rendering for reasoning/thinking transcript cells.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::palette;
use crate::tui::markdown_render;
use crate::tui::ui_text::truncate_line_to_width;

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
const THINKING_COMPLETED_PREVIEW_LINE_LIMIT: usize = 6;
const THINKING_STREAMING_PREVIEW_LINE_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ThinkingVisualState {
    Live,
    Done,
    Idle,
}

#[allow(dead_code)] // Kept for compatibility/tests; live view uses explicit summaries only. (see #3490)
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
    let state = thinking_visual_state(streaming, duration_secs);
    let style = thinking_style();
    // 12% reasoning surface tint over the app ink — the only deliberately
    // warm element in the transcript. Dropped on Ansi-16 terminals where the
    // tint would distort the named palette.
    let depth = cached_color_depth();
    let body_bg = palette::reasoning_surface_tint(depth);
    let body_style = match body_bg {
        Some(bg) => style.italic().bg(bg),
        None => style.italic(),
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
        header_spans.push(Span::styled(format!("{dur:.1}s"), thinking_meta_style()));
    }
    lines.push(Line::from(header_spans));

    let content_width = width.saturating_sub(3).max(1);
    let mut collapsed_without_explicit_summary = false;
    let body_text = if collapsed {
        if streaming {
            // #861 RC4 / #1324: during streaming we don't yet have a
            // completed reasoning block, so `extract_reasoning_summary`
            // is meaningless. Show the raw content and let the
            // truncation logic below keep the *last* `LIMIT` lines so
            // the user sees the model's most recent thinking instead of
            // staring at an empty placeholder.
            content.to_string()
        } else {
            match extract_explicit_reasoning_summary(content) {
                Some(summary) => summary,
                None => {
                    collapsed_without_explicit_summary = true;
                    content.to_string()
                }
            }
        }
    } else {
        content.to_string()
    };
    let mut rendered = if body_text.trim().is_empty() {
        Vec::new()
    } else {
        markdown_render::render_markdown(&body_text, content_width, body_style)
    };
    let mut truncated = false;
    let line_limit = if streaming {
        THINKING_STREAMING_PREVIEW_LINE_LIMIT
    } else if collapsed_without_explicit_summary {
        THINKING_COMPLETED_PREVIEW_LINE_LIMIT
    } else {
        THINKING_SUMMARY_LINE_LIMIT
    };
    if collapsed && rendered.len() > line_limit {
        if streaming {
            // Drop the *head* during streaming so the visible window
            // tracks the live cursor at the bottom.
            let drop = rendered.len() - line_limit;
            rendered.drain(0..drop);
        } else {
            rendered.truncate(line_limit);
        }
        truncated = true;
    }

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
        // Trailing cursor on the very last body line while streaming —
        // signals "still generating" without churning every line.
        if streaming && !low_motion && idx == last_idx {
            spans.push(Span::styled(format!(" {REASONING_CURSOR}"), cursor_style));
        }
        lines.push(Line::from(spans));
    }

    let needs_affordance = collapsed
        && if streaming {
            // #861 RC4 / #1324: during streaming, surface the affordance
            // whenever any head lines have been clipped so the user
            // knows there's more above and how to reach it.
            truncated
        } else {
            truncated || body_text.trim() != content.trim()
        };
    if needs_affordance {
        let label = if streaming {
            "More reasoning in Ctrl+O"
        } else {
            "Space to expand · Full reasoning in Ctrl+O"
        };
        lines.push(Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(label, Style::default().fg(palette::TEXT_MUTED).italic()),
        ]));
    }

    lines
}

pub(super) fn render_hidden_thinking_activity(
    width: u16,
    duration_secs: Option<f32>,
    low_motion: bool,
) -> Vec<Line<'static>> {
    let state = ThinkingVisualState::Live;
    let rail_style = Style::default().fg(thinking_state_accent(state));
    let body_style = thinking_style().italic();
    let content_width = width.saturating_sub(3).max(1) as usize;

    let mut header_spans = vec![
        Span::styled(
            format!("{REASONING_OPENER} "),
            Style::default().fg(thinking_state_accent(state)),
        ),
        Span::styled("reasoning", thinking_title_style()),
        Span::styled(" ", Style::default()),
        Span::styled(thinking_status_label(state), thinking_status_style(state)),
    ];
    if let Some(dur) = duration_secs {
        header_spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
        header_spans.push(Span::styled(format!("{dur:.1}s"), thinking_meta_style()));
    }

    let mut body =
        truncate_line_to_width("reasoning hidden; model is still working", content_width);
    if !low_motion {
        body.push(' ');
        body.push_str(REASONING_CURSOR);
    }

    vec![
        Line::from(header_spans),
        Line::from(vec![
            Span::styled(REASONING_RAIL.to_string(), rail_style),
            Span::styled(body, body_style),
        ]),
    ]
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

fn cached_color_depth() -> palette::ColorDepth {
    *COLOR_DEPTH.get_or_init(palette::ColorDepth::detect)
}
