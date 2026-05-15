//! Header bar widget displaying mode, workspace/model context, and session status.

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

const CONTEXT_WARNING_THRESHOLD_PERCENT: f64 = 85.0;
const CONTEXT_CRITICAL_THRESHOLD_PERCENT: f64 = 95.0;
const CONTEXT_SIGNAL_WIDTH: usize = 4;

/// Milliseconds between status-indicator frame advances. The original
/// `deepseek_squiggle` (v0.3.5 → v0.8.x) used 420 ms; the dot replacement
/// used the same cadence. Keep both at 420 ms so the visual rhythm matches
/// what long-time users remember.
const STATUS_INDICATOR_FRAME_MS: u128 = 420;

/// Whale-cycle frames: 🐳 builds up dots, then surfaces as 🐋. Restored from
/// the original `deepseek_squiggle` in v0.8.30 (removed by commit
/// `1a04659a9` "smoother TUI streaming"). The breaching whale is the
/// punchline at the midpoint of each cycle.
const STATUS_INDICATOR_WHALE_FRAMES: &[&str] = &[
    "🐳", "🐳.", "🐳..", "🐳...", "🐳..", "🐳.", "🐋", "🐋.", "🐋..", "🐋...", "🐋..", "🐋.",
];

/// Geometric replacement frames shipped between v0.8.x and v0.8.29.
const STATUS_INDICATOR_DOT_FRAMES: &[&str] = &["◍", "◉", "◌", "◌", "◉", "◍"];

/// Resolve the current status-indicator frame to render in the header
/// chip cluster.
///
/// `turn_started_at = None` (no active turn) returns the first frame so the
/// chip is *visible* but not animating — it's a chip, not a spinner. As
/// soon as a turn starts, the elapsed time keys the cycle.
///
/// `mode` accepts the canonical names `"whale"`, `"dots"`, `"off"` (any
/// other value is treated as `"whale"` to mirror
/// `StatusIndicatorValue::from(&str)`). `"off"` returns `None` so the
/// caller can hide the chip outright.
#[must_use]
pub fn header_status_indicator_frame(
    turn_started_at: Option<Instant>,
    mode: &str,
) -> Option<&'static str> {
    let frames: &[&str] = match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "hidden" | "false" => return None,
        "dots" | "dot" => STATUS_INDICATOR_DOT_FRAMES,
        // "whale" + aliases + unknown → whale (intentional default).
        _ => STATUS_INDICATOR_WHALE_FRAMES,
    };
    let elapsed_ms = turn_started_at
        .map(|t| t.elapsed().as_millis())
        .unwrap_or(0);
    let idx = (elapsed_ms / STATUS_INDICATOR_FRAME_MS) as usize % frames.len();
    Some(frames[idx])
}

/// Data required to render the header bar.
pub struct HeaderData<'a> {
    pub model: &'a str,
    pub workspace_name: &'a str,
    pub mode: AppMode,
    pub is_streaming: bool,
    pub background: ratatui::style::Color,
    /// Total tokens used in this session (cumulative, for display).
    pub total_tokens: u32,
    /// Context window size for the model (if known).
    pub context_window: Option<u32>,
    /// Accumulated session cost in the active display currency.
    pub session_cost: f64,
    /// Active context input tokens used for context utilization. Callers should
    /// pass a sanitized live-context estimate, not cumulative API usage.
    pub last_prompt_tokens: Option<u32>,
    /// Short label for the current reasoning-effort tier (e.g. "max", "high",
    /// "off"). Rendered as a chip when space allows.
    pub reasoning_effort_label: Option<&'a str>,
    /// Short label for the active provider (e.g. "NIM"). When `None` (the
    /// default-DeepSeek case), no provider chip is rendered. Surfaces the
    /// fact that requests are going somewhere other than DeepSeek's API so
    /// it's visible at a glance after a `/provider nvidia-nim`.
    pub provider_label: Option<&'a str>,
    /// Currently-resolved status indicator glyph rendered as a chip
    /// immediately before the reasoning-effort chip. The caller is
    /// responsible for cycling frames (see [`header_status_indicator_frame`])
    /// so the widget itself stays a pure pre-built render. `None` hides the
    /// chip entirely (e.g., `status_indicator = "off"`).
    pub status_indicator_frame: Option<&'static str>,
}

impl<'a> HeaderData<'a> {
    /// Create header data from common app fields.
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

    /// Attach a short reasoning-effort label for the header chip.
    #[must_use]
    pub fn with_reasoning_effort(mut self, label: Option<&'a str>) -> Self {
        self.reasoning_effort_label = label;
        self
    }

    /// Attach the currently-resolved status indicator frame (e.g. `"🐳.."`).
    /// Pass `None` to hide the chip. Use [`header_status_indicator_frame`]
    /// to compute the right frame for the current turn's elapsed time.
    #[must_use]
    pub fn with_status_indicator(mut self, frame: Option<&'static str>) -> Self {
        self.status_indicator_frame = frame;
        self
    }

    /// Attach a short provider label for the header chip. Pass `None` when on
    /// the default DeepSeek provider so the chip is hidden.
    #[must_use]
    pub fn with_provider(mut self, label: Option<&'a str>) -> Self {
        self.provider_label = label;
        self
    }

    /// Set token/cost fields.
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

/// Header bar widget (1 line height).
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
            AppMode::Agent => "Agent",
            AppMode::Yolo => "Yolo",
            AppMode::Plan => "Plan",
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
            palette::STATUS_WARNING
        } else {
            palette::DEEPSEEK_SKY
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
        spans.push(Span::styled("▰".repeat(filled), Style::default().fg(color)));
        spans.push(Span::styled(
            "▱".repeat(empty),
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
        // Color matches the rest of the live-status cluster (sky), keeping
        // the chip visually grouped with `● Live` and the effort label.
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

    fn effort_chip_spans(&self, include_prefix: bool) -> Vec<Span<'static>> {
        let Some(label) = self.data.reasoning_effort_label else {
            return Vec::new();
        };
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }
        let is_off = trimmed.eq_ignore_ascii_case("off");
        let color = if is_off {
            palette::TEXT_HINT
        } else {
            palette::DEEPSEEK_SKY
        };
        let body = if !include_prefix {
            trimmed.to_string()
        } else if trimmed.eq_ignore_ascii_case("max") || trimmed.eq_ignore_ascii_case("maximum") {
            // Use a non-emoji diamond (U+25C6, always 1 column) instead of an
            // SMP emoji whose rendered width is inconsistent across terminals
            // (cmd/PowerShell, WezTerm, Alacritty). See issue #1314.
            format!("\u{25C6} {trimmed}")
        } else {
            format!("\u{00B7} {trimmed}")
        };
        vec![Span::styled(body, Style::default().fg(color))]
    }

    fn status_variant(
        &self,
        show_stream_label: bool,
        show_percent: bool,
        show_signal: bool,
    ) -> Vec<Span<'static>> {
        let mut spans = Vec::new();

        let provider_spans = self.provider_chip_spans();
        let has_provider = !provider_spans.is_empty();
        if has_provider {
            spans.extend(provider_spans);
        }

        // Status indicator chip (whale 🐳/🐋 or dots ◌/◉ depending on
        // `status_indicator` setting). Sits immediately before the effort
        // chip so the layout reads e.g. `🐳..  ◆ max` — the chip cluster
        // users associate with "where the whale used to be."
        let indicator_spans = self.status_indicator_spans();
        let has_indicator = !indicator_spans.is_empty();
        if has_indicator {
            if has_provider {
                spans.push(Span::raw("  "));
            }
            spans.extend(indicator_spans);
        }

        let effort_spans = self.effort_chip_spans(true);
        let has_effort = !effort_spans.is_empty();
        if has_effort {
            if has_provider || has_indicator {
                spans.push(Span::raw("  "));
            }
            spans.extend(effort_spans);
        }

        if self.data.is_streaming {
            if has_effort || has_provider {
                spans.push(Span::raw("  "));
            }
            spans.push(Span::styled(
                "●",
                Style::default()
                    .fg(palette::DEEPSEEK_SKY)
                    .add_modifier(Modifier::BOLD),
            ));
            if show_stream_label {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    "Live",
                    Style::default().fg(palette::TEXT_SOFT),
                ));
            }
        }

        let context_spans = if show_signal {
            self.context_signal_spans(show_percent)
        } else if show_percent {
            self.context_percent_spans()
        } else {
            Vec::new()
        };
        if !context_spans.is_empty() {
            if !spans.is_empty() {
                spans.push(Span::raw("  "));
            }
            spans.extend(context_spans);
        }

        spans
    }

    /// Compile-time version tag (`v0.8.29`, …). Rendered in the header's
    /// right cluster as the lowest-priority element — see `right_spans`.
    fn version_label() -> String {
        format!("v{}", env!("CARGO_PKG_VERSION"))
    }

    fn version_spans(prefix_existing: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        if prefix_existing {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            Self::version_label(),
            Style::default().fg(palette::TEXT_HINT),
        ));
        spans
    }

    fn right_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        // Width-priority cascade. Each row is a candidate; we pick the
        // first that fits. The version chip is the last thing to drop —
        // once `status_variant(false, false, true)` no longer leaves room
        // for `  v0.8.29`, we fall through to the same status variant
        // without the version chip.
        let pinned = |status: Vec<Span<'static>>| {
            let prefix = !status.is_empty();
            let mut combined = status;
            combined.extend(Self::version_spans(prefix));
            combined
        };

        let candidates = [
            pinned(self.status_variant(true, true, true)),
            pinned(self.status_variant(false, true, true)),
            pinned(self.status_variant(false, true, false)),
            pinned(self.status_variant(false, false, true)),
            self.status_variant(true, true, true),
            self.status_variant(false, true, true),
            self.status_variant(false, true, false),
            self.status_variant(false, false, true),
            Self::version_spans(false),
        ];

        candidates
            .into_iter()
            .find(|spans| Self::span_width(spans) <= max_width)
            .unwrap_or_default()
    }

    fn metadata_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        let workspace = self.data.workspace_name.trim();
        let model = self.data.model.trim();

        if max_width < 4 || (workspace.is_empty() && model.is_empty()) {
            return Vec::new();
        }

        if workspace.is_empty() {
            return vec![Span::styled(
                Self::truncate_to_width(model, max_width),
                Style::default().fg(palette::TEXT_HINT),
            )];
        }

        if model.is_empty() || max_width < 12 {
            return vec![Span::styled(
                Self::truncate_to_width(workspace, max_width),
                Style::default().fg(palette::TEXT_SECONDARY),
            )];
        }

        let separator_width = 3; // " · "
        if workspace.width() + separator_width + model.width() <= max_width {
            return vec![
                Span::styled(
                    workspace.to_string(),
                    Style::default().fg(palette::TEXT_SECONDARY),
                ),
                Span::styled(" · ", Style::default().fg(palette::TEXT_HINT)),
                Span::styled(model.to_string(), Style::default().fg(palette::TEXT_HINT)),
            ];
        }

        let content_width = max_width.saturating_sub(separator_width);
        if content_width < 9 {
            return vec![Span::styled(
                Self::truncate_to_width(workspace, max_width),
                Style::default().fg(palette::TEXT_SECONDARY),
            )];
        }

        let workspace_width = workspace.width();
        let model_width = model.width();
        let total_width = workspace_width + model_width;
        let min_workspace = 4;
        let min_model = 4;

        let proportional_workspace =
            ((content_width as f64 * workspace_width as f64) / total_width as f64).round() as usize;
        let workspace_budget =
            proportional_workspace.clamp(min_workspace, content_width.saturating_sub(min_model));
        let model_budget = content_width.saturating_sub(workspace_budget);

        vec![
            Span::styled(
                Self::truncate_to_width(workspace, workspace_budget),
                Style::default().fg(palette::TEXT_SECONDARY),
            ),
            Span::styled(" · ", Style::default().fg(palette::TEXT_HINT)),
            Span::styled(
                Self::truncate_to_width(model, model_budget),
                Style::default().fg(palette::TEXT_HINT),
            ),
        ]
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
            let fallback = self
                .data
                .mode
                .label()
                .chars()
                .next()
                .unwrap_or('?')
                .to_string();
            return vec![Span::styled(fallback, mode_style)];
        }

        let mut spans = vec![Span::styled(mode_label.to_string(), mode_style)];
        let metadata_width = max_width
            .saturating_sub(mode_label.width())
            .saturating_sub(2);
        let metadata = if metadata_width >= 4 {
            self.metadata_spans(metadata_width)
        } else {
            Vec::new()
        };

        if !metadata.is_empty() {
            spans.push(Span::raw("  "));
            spans.extend(metadata);
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
        let right_budget = available.saturating_sub(6);
        let right_spans = self.right_spans(right_budget);
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

        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(self.data.background));
        paragraph.render(area, buf);
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
    fn wide_header_shows_plain_mode_and_single_metadata_cluster() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            ),
            72,
        );

        assert!(rendered.contains("Agent"));
        assert!(rendered.contains("deepseek-tui"));
        assert!(rendered.contains("deepseek-v4-pro"));
        assert!(!rendered.contains("Plan"));
        assert!(!rendered.contains("Yolo"));
    }

    #[test]
    fn header_renders_version_chip_when_width_allows() {
        // At a generous width the header must surface the runtime version
        // — users repeatedly ask for it in the live UI (vs only via
        // `deepseek --version` / `/status`).
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            ),
            120,
        );
        let expected = format!("v{}", env!("CARGO_PKG_VERSION"));
        assert!(
            rendered.contains(&expected),
            "expected version chip `{expected}` in header: {rendered:?}",
        );
    }

    #[test]
    fn narrow_header_drops_version_chip_before_dropping_mode() {
        // Very tight width budget — the version is among the first
        // chips to disappear; the mode label must still render.
        let rendered = render_header(
            HeaderData::new(
                AppMode::Yolo,
                "deepseek-v4-pro",
                "deepseek-tui",
                true,
                palette::DEEPSEEK_INK,
            )
            .with_usage(1_000, Some(128_000), 0.0, Some(2_000)),
            12,
        );
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));
        assert!(
            !rendered.contains(&version),
            "version chip should drop under width pressure: {rendered:?}",
        );
        assert!(
            rendered.contains("Yolo") || rendered.contains('Y'),
            "mode label must survive: {rendered:?}",
        );
    }

    #[test]
    fn streaming_header_integrates_live_state_with_context_signal() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Plan,
                "deepseek-v4-pro",
                "workspace",
                true,
                palette::DEEPSEEK_INK,
            )
            .with_usage(42_000, Some(128_000), 0.0, Some(48_000)),
            72,
        );

        assert!(rendered.contains("Live"));
        assert!(rendered.contains("38%"));
        assert!(rendered.contains("▰"));
    }

    #[test]
    fn narrow_header_keeps_context_percent_visible() {
        let rendered = render_header(
            HeaderData::new(AppMode::Agent, "", "", true, palette::DEEPSEEK_INK).with_usage(
                0,
                Some(128_000),
                0.0,
                Some(48_000),
            ),
            14,
        );

        assert!(rendered.contains('%'));
    }

    #[test]
    fn narrow_header_falls_back_to_mode_without_rendering_all_modes() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Yolo,
                "deepseek-v4-flash",
                "repo",
                true,
                palette::DEEPSEEK_INK,
            )
            .with_usage(1_000, Some(10_000), 0.0, Some(4_000)),
            8,
        );

        assert!(rendered.trim_start().starts_with('Y'));
        assert!(!rendered.contains("Plan"));
        assert!(!rendered.contains("Agent"));
    }

    #[test]
    fn header_hides_context_signal_when_usage_snapshot_is_missing() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-flash",
                "repo",
                false,
                palette::DEEPSEEK_INK,
            ),
            48,
        );

        assert!(!rendered.contains('%'));
        assert!(!rendered.contains("▰"));
    }

    #[test]
    fn header_caps_context_signal_at_hundred_percent() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-flash",
                "repo",
                false,
                palette::DEEPSEEK_INK,
            )
            .with_usage(1_000, Some(128_000), 0.0, Some(320_000)),
            48,
        );

        assert!(rendered.contains("100%"));
        assert!(!rendered.contains("250%"));
    }

    #[test]
    fn header_shows_provider_chip_when_set() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-ai/deepseek-v4-flash",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            )
            .with_provider(Some("NIM")),
            72,
        );
        assert!(
            rendered.contains("NIM"),
            "expected NIM chip in header, got: {rendered}"
        );
    }

    #[test]
    fn header_hides_provider_chip_when_default_deepseek() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            ),
            72,
        );
        // Sanity: no `NIM` text leaks in when provider is None.
        assert!(!rendered.contains("NIM"));
    }

    #[test]
    fn whale_indicator_idle_frame_is_first_whale_glyph() {
        // No active turn = no animation, just the calm 🐳 glyph sitting
        // next to the effort chip.
        let frame = super::header_status_indicator_frame(None, "whale");
        assert_eq!(frame, Some("🐳"));
    }

    #[test]
    fn whale_indicator_advances_through_frames_then_breaches() {
        use std::thread::sleep;
        use std::time::Duration;
        let start = std::time::Instant::now();
        // Frame 0 immediately.
        assert_eq!(
            super::header_status_indicator_frame(Some(start), "whale"),
            Some("🐳")
        );
        // After ~420ms one tick has elapsed → frame 1.
        sleep(Duration::from_millis(430));
        assert_eq!(
            super::header_status_indicator_frame(Some(start), "whale"),
            Some("🐳.")
        );
    }

    #[test]
    fn dots_indicator_uses_geometric_frames() {
        let frame = super::header_status_indicator_frame(None, "dots");
        assert_eq!(frame, Some("\u{25CD}"));
    }

    #[test]
    fn off_indicator_returns_none_so_chip_is_hidden() {
        assert!(super::header_status_indicator_frame(None, "off").is_none());
        // Aliases mirror the parser in Settings.
        assert!(super::header_status_indicator_frame(None, "none").is_none());
        assert!(super::header_status_indicator_frame(None, "hidden").is_none());
        assert!(super::header_status_indicator_frame(None, "false").is_none());
    }

    #[test]
    fn unknown_indicator_mode_defaults_to_whale() {
        // We'd rather restore the whale on a typo than silently hide the
        // chip — matches `StatusIndicatorValue::from(&str)`.
        let frame = super::header_status_indicator_frame(None, "wahel-typo");
        assert_eq!(frame, Some("🐳"));
    }

    #[test]
    fn header_renders_whale_chip_next_to_effort_label() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            )
            .with_reasoning_effort(Some("max"))
            .with_status_indicator(Some("🐳")),
            72,
        );
        assert!(
            rendered.contains("🐳"),
            "expected whale chip in header, got: {rendered}"
        );
        assert!(
            rendered.contains("max"),
            "expected effort chip preserved, got: {rendered}"
        );
        // Whale appears before "max" — sanity-check ordering by index.
        let whale_idx = rendered.find("🐳").expect("whale present");
        let max_idx = rendered.find("max").expect("max present");
        assert!(
            whale_idx < max_idx,
            "expected whale to render before effort label, got: {rendered}"
        );
    }

    #[test]
    fn header_hides_whale_chip_when_status_indicator_off() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "deepseek-tui",
                false,
                palette::DEEPSEEK_INK,
            )
            .with_reasoning_effort(Some("max"))
            .with_status_indicator(None),
            72,
        );
        assert!(!rendered.contains("🐳"));
        assert!(!rendered.contains("🐋"));
        assert!(rendered.contains("max"));
    }
}
