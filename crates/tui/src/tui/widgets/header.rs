//! Header widget — Claude Code style: minimal one-line status bar.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::palette;
use crate::tui::app::AppMode;

use super::Renderable;

const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 60.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_SIGNAL_WIDTH: usize = 6;

pub struct HeaderData<'a> {
    pub model: &'a str,
    pub workspace_name: &'a str,
    pub mode: AppMode,
    pub is_streaming: bool,
    #[allow(dead_code)]
    pub background: ratatui::style::Color,
    pub total_tokens: u32,
    pub context_window: Option<u32>,
    pub session_cost: f64,
    pub last_prompt_tokens: Option<u32>,
    pub reasoning_effort_label: Option<&'a str>,
    pub provider_label: Option<&'a str>,
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
            model, workspace_name, mode, is_streaming, background,
            total_tokens: 0, context_window: None, session_cost: 0.0,
            last_prompt_tokens: None, reasoning_effort_label: None, provider_label: None,
        }
    }
    #[must_use]
    pub fn with_reasoning_effort(mut self, label: Option<&'a str>) -> Self { self.reasoning_effort_label = label; self }
    #[must_use]
    pub fn with_provider(mut self, label: Option<&'a str>) -> Self { self.provider_label = label; self }
    #[must_use]
    pub fn with_usage(mut self, total_tokens: u32, context_window: Option<u32>, session_cost: f64, active_context_input_tokens: Option<u32>) -> Self {
        self.total_tokens = total_tokens; self.context_window = context_window;
        self.session_cost = session_cost; self.last_prompt_tokens = active_context_input_tokens; self
    }
}

pub struct HeaderWidget<'a> { data: HeaderData<'a> }

impl<'a> HeaderWidget<'a> {
    #[must_use]
    pub fn new(data: HeaderData<'a>) -> Self { Self { data } }

    fn mode_color(mode: AppMode) -> Color {
        match mode { AppMode::Agent => palette::MODE_AGENT, AppMode::Yolo => palette::MODE_YOLO, AppMode::Plan => palette::MODE_PLAN }
    }
    fn mode_name(mode: AppMode) -> &'static str {
        match mode { AppMode::Agent => "agent", AppMode::Yolo => "yolo", AppMode::Plan => "plan" }
    }
    fn span_width(spans: &[Span<'_>]) -> usize { spans.iter().map(|span| span.content.width()).sum() }

    fn context_percent(&self) -> Option<f64> {
        let used = f64::from(self.data.last_prompt_tokens?);
        let max = f64::from(self.data.context_window?);
        if max <= 0.0 { return None; }
        Some((used / max * 100.0).clamp(0.0, 100.0))
    }

    fn context_color(percent: f64) -> Color {
        if percent >= CONTEXT_CRITICAL_THRESHOLD_PERCENT {
            palette::STATUS_ERROR
        } else if percent >= CONTEXT_WARNING_THRESHOLD_PERCENT {
            let t = ((percent - CONTEXT_WARNING_THRESHOLD_PERCENT) / (CONTEXT_CRITICAL_THRESHOLD_PERCENT - CONTEXT_WARNING_THRESHOLD_PERCENT)).clamp(0.0, 1.0);
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
        let Some(percent) = self.context_percent() else { return Vec::new(); };
        let color = Self::context_color(percent);
        let filled = ((percent / 100.0) * CONTEXT_SIGNAL_WIDTH as f64).ceil().clamp(0.0, CONTEXT_SIGNAL_WIDTH as f64) as usize;
        let empty = CONTEXT_SIGNAL_WIDTH.saturating_sub(filled);
        let mut spans = Vec::new();
        if show_percent { spans.push(Span::styled(format!("{percent:.0}%"), Style::default().fg(color))); spans.push(Span::raw(" ")); }
        spans.push(Span::styled("\u{25B0}".repeat(filled), Style::default().fg(color)));
        spans.push(Span::styled("\u{25B1}".repeat(empty), Style::default().fg(palette::BORDER_COLOR)));
        spans
    }
}

impl Renderable for HeaderWidget<'_> {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 { return; }
        let available = area.width as usize;

        // Right: context bar always. Streaming indicator when live.
        let right_spans = {
            let ctx = self.context_signal_spans(true);
            if self.data.is_streaming {
                let mut spans = vec![Span::styled("\u{23FA}", Style::default().fg(palette::ACCENT_TOOL_LIVE).add_modifier(Modifier::BOLD))];
                if !ctx.is_empty() { spans.push(Span::raw("  ")); }
                spans.extend(ctx); spans
            } else { ctx }
        };
        let right_width = Self::span_width(&right_spans);

        // Left: minimal mode · model.
        let left_budget = available.saturating_sub(right_width + usize::from(right_width > 0));
        let left_spans = {
            let mode_label = Self::mode_name(self.data.mode);
            let mode_color = Self::mode_color(self.data.mode);
            let mut spans = vec![Span::styled(mode_label.to_string(), Style::default().fg(mode_color).add_modifier(Modifier::BOLD))];
            let model = self.data.model.trim();
            if !model.is_empty() && left_budget > mode_label.width() + 4 {
                spans.push(Span::styled(format!(" · {model}"), Style::default().fg(palette::TEXT_DIM)));
            }
            spans
        };
        let left_width = Self::span_width(&left_spans);
        let spacer = available.saturating_sub(left_width + right_width);
        let mut all = left_spans;
        if spacer > 0 { all.push(Span::raw(" ".repeat(spacer))); }
        all.extend(right_spans);
        Paragraph::new(Line::from(all)).render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 { 1 }
}
