//! Header widget — Claude Code style: minimal one-line status bar.

use std::time::Instant;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::palette;
use crate::tui::app::AppMode;

use super::Renderable;

const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 60.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_SIGNAL_WIDTH: usize = 6;
const STATUS_INDICATOR_FRAME_MS: u128 = 420;
const STATUS_INDICATOR_WHALE_FRAMES: &[&str] = &[
    "🐳", "🐳.", "🐳..", "🐳...", "🐳..", "🐳.", "🐋", "🐋.", "🐋..", "🐋...", "🐋..", "🐋.",
];
const STATUS_INDICATOR_DOT_FRAMES: &[&str] = &["◍", "◉", "◌", "◌", "◉", "◍"];

#[must_use]
pub fn header_status_indicator_frame(
    turn_started_at: Option<Instant>,
    mode: &str,
) -> Option<&'static str> {
    let frames: &[&str] = match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "hidden" | "false" => return None,
        "dots" | "dot" => STATUS_INDICATOR_DOT_FRAMES,
        _ => STATUS_INDICATOR_WHALE_FRAMES,
    };
    let elapsed_ms = turn_started_at
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    let idx = (elapsed_ms / STATUS_INDICATOR_FRAME_MS) as usize % frames.len();
    Some(frames[idx])
}

pub struct HeaderData<'a> {
    pub model: &'a str,
    pub workspace_name: &'a str,
    pub mode: AppMode,
    pub is_streaming: bool,
    pub background: ratatui::style::Color,
    pub total_tokens: u32,
    pub context_window: Option<u32>,
    pub session_cost: f64,
    pub last_prompt_tokens: Option<u32>,
    pub reasoning_effort_label: Option<&'a str>,
    pub provider_label: Option<&'a str>,
    pub status_indicator_frame: Option<&'static str>,
}

impl<'a> HeaderData<'a> {
    #[must_use]
    pub fn new(
        mode: AppMode,
        model: &'a str,
        workspace_name: &'a str,
        is_streaming: bool,
        background: ratatui::style::Color,
    ) -> Self {
        Self {
            model,
            workspace_name,
            mode,
            is_streaming,
            background,
            total_tokens: 0,
            context_window: None,
            session_cost: 0.0,
            last_prompt_tokens: None,
            reasoning_effort_label: None,
            provider_label: None,
            status_indicator_frame: None,
        }
    }

    #[must_use]
    pub fn with_reasoning_effort(mut self, label: Option<&'a str>) -> Self {
        self.reasoning_effort_label = label;
        self
    }

    #[must_use]
    pub fn with_status_indicator(mut self, frame: Option<&'static str>) -> Self {
        self.status_indicator_frame = frame;
        self
    }

    #[must_use]
    pub fn with_provider(mut self, label: Option<&'a str>) -> Self {
        self.provider_label = label;
        self
    }

    #[must_use]
    pub fn with_usage(
        mut self,
        total_tokens: u32,
        context_window: Option<u32>,
        session_cost: f64,
        active_context_input_tokens: Option<u32>,
    ) -> Self {
        self.total_tokens = total_tokens;
        self.context_window = context_window;
        self.session_cost = session_cost;
        self.last_prompt_tokens = active_context_input_tokens;
        self
    }
}

pub struct HeaderWidget<'a> {
    data: HeaderData<'a>,
}

impl<'a> HeaderWidget<'a> {
    #[must_use]
    pub fn new(data: HeaderData<'a>) -> Self {
        Self { data }
    }

    fn mode_color(mode: AppMode) -> Color {
        match mode {
            AppMode::Agent => palette::MODE_AGENT,
            AppMode::Yolo => palette::MODE_YOLO,
            AppMode::Plan => palette::MODE_PLAN,
        }
    }

    fn mode_name(mode: AppMode) -> &'static str {
        match mode {
            AppMode::Agent => "agent",
            AppMode::Yolo => "yolo",
            AppMode::Plan => "plan",
        }
    }

    fn span_width(spans: &[Span<'_>]) -> usize {
        spans.iter().map(|span| span.content.width()).sum()
    }

    fn truncate_to_width(text: &str, max_width: usize) -> String {
        const ELLIPSIS: &str = "...";
        let ellipsis_width = ELLIPSIS.width();

        if text.width() <= max_width {
            return text.to_string();
        }
        if max_width == 0 {
            return String::new();
        }
        if max_width <= ellipsis_width {
            return ".".repeat(max_width);
        }

        let mut truncated = String::new();
        let mut width = 0;
        for ch in text.chars() {
            let ch_width = ch.width().unwrap_or(0);
            if width + ch_width + ellipsis_width > max_width {
                break;
            }
            truncated.push(ch);
            width += ch_width;
        }
        truncated.push_str(ELLIPSIS);
        truncated
    }

    fn context_percent(&self) -> Option<f64> {
        let used = f64::from(self.data.last_prompt_tokens?);
        let max = f64::from(self.data.context_window?);
        if max <= 0.0 {
            return None;
        }
        Some((used / max * 100.0).clamp(0.0, 100.0))
    }

    fn context_color(percent: f64) -> Color {
        if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
            palette::STATUS_ERROR
        } else if percent >= CONTEXT_WARNING_THRESHOLD_PERCENT {
            let t = ((percent - CONTEXT_WARNING_THRESHOLD_PERCENT)
                / (CONTEXT_CRITICAL_THRESHOLD_PERCENT - CONTEXT_WARNING_THRESHOLD_PERCENT))
                .clamp(0.0, 1.0);
            let r = (121.0 + (248.0 - 121.0) * t) as u8;
            let g = (192.0 + (81.0 - 192.0) * t) as u8;
            let b = (255.0 + (73.0 - 255.0) * t) as u8;
            Color::Rgb(r, g, b)
        } else {
            let t = (percent / CONTEXT_WARNING_THRESHOLD_PERCENT).clamp(0.0, 1.0);
            let r = (63.0 + (121.0 - 63.0) * t) as u8;
            let g = (185.0 + (192.0 - 185.0) * t) as u8;
            let b = (80.0 + (255.0 - 80.0) * t) as u8;
            Color::Rgb(r, g, b)
        }
    }

    fn context_signal_spans(&self, show_percent: bool) -> Vec<Span<'static>> {
        let Some(percent) = self.context_percent() else {
            return Vec::new();
        };
        let color = Self::context_color(percent);
        let filled = ((percent / 100.0) * CONTEXT_SIGNAL_WIDTH as f64)
            .ceil()
            .clamp(0.0, CONTEXT_SIGNAL_WIDTH as f64) as usize;
        let empty = CONTEXT_SIGNAL_WIDTH.saturating_sub(filled);
        let mut spans = Vec::new();
        if show_percent {
            spans.push(Span::styled(
                format!("{percent:.0}%"),
                Style::default().fg(color),
            ));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            "\u{25B0}".repeat(filled),
            Style::default().fg(color),
        ));
        spans.push(Span::styled(
            "\u{25B1}".repeat(empty),
            Style::default().fg(palette::BORDER_COLOR),
        ));
        spans
    }

    fn context_percent_spans(&self) -> Vec<Span<'static>> {
        let Some(percent) = self.context_percent() else {
            return Vec::new();
        };

        vec![Span::styled(
            format!("{percent:.0}%"),
            Style::default().fg(Self::context_color(percent)),
        )]
    }

    fn status_indicator_spans(&self) -> Vec<Span<'static>> {
        let Some(frame) = self.data.status_indicator_frame else {
            return Vec::new();
        };
        vec![Span::styled(
            frame.to_string(),
            Style::default().fg(palette::DEEPSEEK_SKY),
        )]
    }

    fn provider_chip_spans(&self) -> Vec<Span<'static>> {
        let Some(label) = self.data.provider_label else {
            return Vec::new();
        };
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        vec![Span::styled(
            trimmed.to_string(),
            Style::default()
                .fg(palette::DEEPSEEK_SKY)
                .add_modifier(Modifier::BOLD),
        )]
    }

    fn effort_chip_spans(&self) -> Vec<Span<'static>> {
        let Some(label) = self.data.reasoning_effort_label else {
            return Vec::new();
        };
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let color = if trimmed.eq_ignore_ascii_case("off") {
            palette::TEXT_HINT
        } else {
            palette::DEEPSEEK_SKY
        };
        let body = if trimmed.eq_ignore_ascii_case("max") || trimmed.eq_ignore_ascii_case("maximum")
        {
            format!("\u{25C6} {trimmed}")
        } else {
            format!("\u{00B7} {trimmed}")
        };
        vec![Span::styled(body, Style::default().fg(color))]
    }

    fn live_spans(&self, show_label: bool) -> Vec<Span<'static>> {
        if !self.data.is_streaming {
            return Vec::new();
        }
        let mut spans = vec![Span::styled(
            "\u{23FA}",
            Style::default()
                .fg(palette::ACCENT_TOOL_LIVE)
                .add_modifier(Modifier::BOLD),
        )];
        if show_label {
            spans.push(Span::styled(
                " live",
                Style::default().fg(palette::TEXT_SOFT),
            ));
        }
        spans
    }

    fn join_spans(parts: Vec<Vec<Span<'static>>>) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for part in parts {
            if part.is_empty() {
                continue;
            }
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.extend(part);
        }
        spans
    }

    fn status_variant(
        &self,
        show_stream_label: bool,
        show_percent: bool,
        show_signal: bool,
    ) -> Vec<Span<'static>> {
        let context = if show_signal {
            self.context_signal_spans(show_percent)
        } else if show_percent {
            self.context_percent_spans()
        } else {
            Vec::new()
        };

        Self::join_spans(vec![
            self.provider_chip_spans(),
            self.status_indicator_spans(),
            self.effort_chip_spans(),
            self.live_spans(show_stream_label),
            context,
        ])
    }

    fn right_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        if max_width == 0 {
            return Vec::new();
        }

        let candidates = [
            self.status_variant(true, true, true),
            self.status_variant(false, true, true),
            self.status_variant(false, true, false),
            self.status_variant(false, false, true),
            self.status_variant(false, false, false),
            self.context_signal_spans(true),
            self.context_percent_spans(),
        ];

        candidates
            .into_iter()
            .find(|spans| Self::span_width(spans) <= max_width)
            .unwrap_or_default()
    }

    fn left_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        if max_width == 0 {
            return Vec::new();
        }

        let mode_label = Self::mode_name(self.data.mode);
        let mode_style = Style::default()
            .fg(Self::mode_color(self.data.mode))
            .add_modifier(Modifier::BOLD);

        if max_width < mode_label.width() {
            let fallback = mode_label.chars().next().unwrap_or('?').to_string();
            return vec![Span::styled(fallback, mode_style)];
        }

        let mut spans = vec![Span::styled(mode_label.to_string(), mode_style)];
        let model = self.data.model.trim();
        let workspace = self.data.workspace_name.trim();
        let detail = if model.is_empty() { workspace } else { model };
        let separator_width = 3;
        let used = mode_label.width();
        let remaining = max_width.saturating_sub(used);
        if !detail.is_empty() && remaining > separator_width + 3 {
            let detail_width = remaining.saturating_sub(separator_width);
            spans.push(Span::styled(
                format!(" · {}", Self::truncate_to_width(detail, detail_width)),
                Style::default().fg(palette::TEXT_DIM),
            ));
        }
        spans
    }
}

impl Renderable for HeaderWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let available = area.width as usize;
        let right_spans = self.right_spans(available.saturating_sub(2));
        let right_width = Self::span_width(&right_spans);
        let spacer_min = usize::from(right_width > 0);
        let left_budget = available.saturating_sub(right_width + spacer_min);
        let left_spans = self.left_spans(left_budget);
        let left_width = Self::span_width(&left_spans);
        let spacer_width = available.saturating_sub(left_width + right_width);

        let mut spans = left_spans;
        if spacer_width > 0 {
            spans.push(Span::raw(" ".repeat(spacer_width)));
        }
        spans.extend(right_spans);

        Paragraph::new(Line::from(spans))
            .style(Style::default().bg(self.data.background))
            .render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::{HeaderData, HeaderWidget, Renderable};
    use crate::palette;
    use crate::tui::app::AppMode;
    use ratatui::{buffer::Buffer, layout::Rect};

    fn render_header(data: HeaderData<'_>, width: u16) -> String {
        let widget = HeaderWidget::new(data);
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        (0..width).map(|x| buf[(x, 0)].symbol()).collect::<String>()
    }

    #[test]
    fn header_renders_minimal_mode_model_and_chips() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            )
            .with_reasoning_effort(Some("max"))
            .with_provider(Some("NIM"))
            .with_status_indicator(Some("🐳")),
            96,
        );

        assert!(rendered.contains("agent"));
        assert!(rendered.contains("deepseek-v4-pro"));
        assert!(rendered.contains("NIM"));
        assert!(rendered.contains("🐳"));
        assert!(rendered.contains("max"));
    }

    #[test]
    fn status_indicator_parser_accepts_off_aliases() {
        assert!(super::header_status_indicator_frame(None, "off").is_none());
        assert!(super::header_status_indicator_frame(None, "none").is_none());
        assert!(super::header_status_indicator_frame(None, "hidden").is_none());
        assert!(super::header_status_indicator_frame(None, "false").is_none());
    }

    #[test]
    fn status_indicator_defaults_to_whale() {
        assert_eq!(
            super::header_status_indicator_frame(None, "typo"),
            Some("🐳")
        );
    }
}
