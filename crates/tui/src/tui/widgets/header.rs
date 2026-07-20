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

/// Frames retained only for the explicitly selected classic treatment.
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
    if matches!(
        mode.trim().to_ascii_lowercase().as_str(),
        "cw" | "mark" | "text"
    ) {
        return Some("cw");
    }
    let frames: &[&str] = match mode.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "hidden" | "false" => return None,
        "dots" | "dot" => STATUS_INDICATOR_DOT_FRAMES,
        "whale" | "🐳" | "🐋" => STATUS_INDICATOR_WHALE_FRAMES,
        // Unknown values keep the owned typographic mark visible.
        _ => return Some("cw"),
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
        _is_streaming: bool,
        background: ratatui::style::Color,
    ) -> Self {
        Self {
            model,
            workspace_name,
            mode,
            background,
            total_tokens: 0,
            context_window: None,
            session_cost: 0.0,
            last_prompt_tokens: None,
            reasoning_effort_label: None,
            provider_label: None,
            status_indicator_frame: Some("cw"),
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
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => palette::MODE_AGENT,
            AppMode::Plan => palette::MODE_PLAN,
            AppMode::Operate => palette::MODE_OPERATE,
        }
    }

    fn mode_name(mode: AppMode) -> &'static str {
        match mode {
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => "Act",
            AppMode::Plan => "Plan",
            AppMode::Operate => "Operate",
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
            palette::WHALE_INFO
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
        let color = if frame == "cw" {
            palette::WHALE_HUMAN
        } else {
            palette::WHALE_INFO
        };
        vec![Span::styled(
            frame.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )]
    }

    #[allow(dead_code)]
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
                .fg(palette::WHALE_INFO)
                .add_modifier(Modifier::BOLD),
        )]
    }

    #[allow(dead_code)]
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
            palette::WHALE_INFO
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
        _show_stream_label: bool,
        show_percent: bool,
        show_signal: bool,
    ) -> Vec<Span<'static>> {
        let context_spans = if show_signal {
            self.context_signal_spans(show_percent)
        } else if show_percent {
            self.context_percent_spans()
        } else {
            Vec::new()
        };
        if !context_spans.is_empty() {
            return context_spans;
        }
        Vec::new()
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

    #[allow(dead_code)]
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

        let mode_label = Self::mode_name(self.data.mode).to_ascii_lowercase();
        let mode_style = Style::default()
            .fg(Self::mode_color(self.data.mode))
            .add_modifier(Modifier::BOLD);
        let mut spans = self.status_indicator_spans();
        let used = Self::span_width(&spans);

        let provider = self.data.provider_label.unwrap_or("").trim();
        let model = self.data.model.trim();
        let route = if provider.is_empty() {
            model.to_string()
        } else {
            format!("{provider}:{model}")
        };
        let effort = self.data.reasoning_effort_label.unwrap_or("").trim();
        let mode_width = 3 + mode_label.width();
        let full_effort_width = 3 + effort.width();
        let compact_effort = Self::compact_effort_label(effort);
        let compact_effort_width = 3 + compact_effort.width();
        let effort = if effort.is_empty() {
            String::new()
        } else if used + mode_width + full_effort_width <= max_width {
            effort.to_string()
        } else if used + mode_width + compact_effort_width <= max_width {
            compact_effort.to_string()
        } else {
            String::new()
        };
        // Reserve mode + effort before granting any width to the route. A
        // long provider/model identity may truncate, but it cannot silently
        // evict the requested/effective effort receipt.
        let fixed_width = mode_width + usize::from(!effort.is_empty()) * (3 + effort.width());
        let status_route_gap = usize::from(used > 0);
        let route_budget = max_width.saturating_sub(used + fixed_width + status_route_gap);
        let route = if route_budget >= 4 {
            Self::truncate_to_width(&route, route_budget)
        } else {
            String::new()
        };

        if !spans.is_empty() && !route.is_empty() {
            spans.push(Span::raw(" "));
        }
        if !route.is_empty() {
            spans.push(Span::styled(route, Style::default().fg(palette::TEXT_HINT)));
        }
        if Self::span_width(&spans) + 3 + mode_label.width() <= max_width {
            spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
            spans.push(Span::styled(mode_label, mode_style));
        }
        if !effort.is_empty() && Self::span_width(&spans) + 3 + effort.width() <= max_width {
            spans.push(Span::styled(" · ", Style::default().fg(palette::TEXT_DIM)));
            spans.push(Span::styled(
                effort.to_string(),
                Style::default().fg(palette::WHALE_INFO),
            ));
        }
        spans
    }

    fn compact_effort_label(label: &str) -> &'static str {
        let effective = label
            .rsplit_once('→')
            .map_or(label, |(_, effective)| effective);
        let effective = effective
            .rsplit_once(':')
            .map_or(effective, |(_, effective)| effective)
            .trim()
            .to_ascii_lowercase();
        match effective.as_str() {
            "off" => "o",
            "low" => "l",
            "med" | "medium" => "m",
            "high" => "h",
            "max" | "maximum" | "xhigh" => "x",
            "auto" => "a",
            _ => "·",
        }
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
    use unicode_width::UnicodeWidthStr;

    fn render_header(data: HeaderData<'_>, width: u16) -> String {
        let widget = HeaderWidget::new(data);
        let area = Rect::new(0, 0, width, 1);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        (0..width).map(|x| buf[(x, 0)].symbol()).collect::<String>()
    }

    fn render_left(data: HeaderData<'_>, width: usize) -> String {
        HeaderWidget::new(data)
            .left_spans(width)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn wide_header_shows_plain_mode_and_single_metadata_cluster() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
            ),
            72,
        );

        // Wave 7: the Agent mode chip reads "Act".
        assert!(rendered.contains("cw"));
        assert!(rendered.contains("act"));
        assert!(rendered.contains("deepseek-v4-pro"));
        assert!(!rendered.contains("Plan"));
        assert!(!rendered.contains("Yolo"));
    }

    #[test]
    fn header_renders_version_chip_when_width_allows() {
        // At a generous width the header must surface the runtime version
        // — users repeatedly ask for it in the live UI (vs only via
        // `codewhale --version` / `/status`).
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
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
        // YOLO is invisible Act+Bypass shorthand, so the chip reads "Act".
        let rendered = render_header(
            HeaderData::new(
                AppMode::Yolo,
                "deepseek-v4-pro",
                "codewhale-tui",
                true,
                palette::WHALE_BG,
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
            rendered.contains("act") || rendered.contains('a'),
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
                palette::WHALE_BG,
            )
            .with_usage(42_000, Some(128_000), 0.0, Some(48_000)),
            72,
        );

        assert!(!rendered.contains("Live"));
        assert!(rendered.contains("38%"));
        assert!(rendered.contains("▰"));
    }

    #[test]
    fn narrow_header_keeps_context_percent_visible() {
        let rendered = render_header(
            HeaderData::new(AppMode::Agent, "", "", true, palette::WHALE_BG).with_usage(
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
                palette::WHALE_BG,
            )
            .with_usage(1_000, Some(10_000), 0.0, Some(4_000)),
            8,
        );

        // YOLO renders as Act; under extreme width pressure only the first
        // glyph of the mode chip remains.
        assert!(rendered.trim_start().starts_with("cw"));
        assert!(rendered.contains("act"));
        assert!(!rendered.contains("Plan"));
        assert!(!rendered.contains("Operate"));
    }

    #[test]
    fn header_hides_context_signal_when_usage_snapshot_is_missing() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-flash",
                "repo",
                false,
                palette::WHALE_BG,
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
                palette::WHALE_BG,
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
                "codewhale-tui",
                false,
                palette::WHALE_BG,
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
                "codewhale-tui",
                false,
                palette::WHALE_BG,
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
    fn cw_indicator_is_static_and_typographic() {
        assert_eq!(super::header_status_indicator_frame(None, "cw"), Some("cw"));
        assert_eq!(
            super::header_status_indicator_frame(Some(std::time::Instant::now()), "cw"),
            Some("cw")
        );
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
    fn unknown_indicator_mode_defaults_to_cw_mark() {
        let frame = super::header_status_indicator_frame(None, "wahel-typo");
        assert_eq!(frame, Some("cw"));
    }

    #[test]
    fn header_renders_whale_chip_next_to_effort_label() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
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
    fn route_truncation_reserves_requested_effective_effort() {
        let rendered = render_left(
            HeaderData::new(
                AppMode::Agent,
                "a-very-long-model-route-that-must-truncate",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
            )
            .with_provider(Some("xiaomi-mimo"))
            .with_reasoning_effort(Some("low→high"))
            .with_status_indicator(Some("cw")),
            28,
        );

        assert!(rendered.contains("low→high"), "{rendered:?}");
        assert!(rendered.contains("act"), "{rendered:?}");
        assert!(rendered.width() <= 28, "{rendered:?}");
    }

    #[test]
    fn narrow_header_uses_one_glyph_effective_effort() {
        let rendered = render_left(
            HeaderData::new(
                AppMode::Agent,
                "a-very-long-model-route",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
            )
            .with_reasoning_effort(Some("low→high"))
            .with_status_indicator(Some("cw")),
            14,
        );

        assert!(!rendered.contains("low→high"), "{rendered:?}");
        assert!(rendered.ends_with(" · h"), "{rendered:?}");
        assert!(rendered.width() <= 14, "{rendered:?}");
    }

    #[test]
    fn cw_indicator_keeps_the_human_brand_lane_distinct_from_live_frames() {
        let cw = HeaderWidget::new(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
            )
            .with_status_indicator(Some("cw")),
        )
        .status_indicator_spans();
        let live = HeaderWidget::new(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
            )
            .with_status_indicator(Some("··")),
        )
        .status_indicator_spans();

        assert_eq!(cw[0].style.fg, Some(palette::WHALE_HUMAN));
        assert_eq!(live[0].style.fg, Some(palette::WHALE_INFO));
        assert_ne!(cw[0].style.fg, live[0].style.fg);
    }

    #[test]
    fn header_hides_whale_chip_when_status_indicator_off() {
        let rendered = render_header(
            HeaderData::new(
                AppMode::Agent,
                "deepseek-v4-pro",
                "codewhale-tui",
                false,
                palette::WHALE_BG,
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
