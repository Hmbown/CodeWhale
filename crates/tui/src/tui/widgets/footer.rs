//! Footer bar widget displaying mode, status, model, and auxiliary chips.
//!
//! `FooterWidget` is a pure render of a [`FooterProps`] struct: all content
//! (labels, colors, span clusters) is computed once per redraw at a higher
//! level, then `FooterWidget::new(props).render(area, buf)` paints the
//! result. The widget owns no `App` knowledge; this mirrors the layout used
//! by `HeaderWidget` (and Codex's `bottom_pane::footer::Footer`).

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::{App, AppMode};

use super::Renderable;

/// Pre-computed data the footer needs to render.
///
/// All fields are owned `String` / `Vec<Span<'static>>` values so the props
/// can be built once per redraw and then handed to a borrow-free widget.
#[derive(Debug, Clone)]
pub struct FooterProps {
    /// The current model identifier shown after the mode chip.
    pub model: String,
    /// `"agent"` / `"yolo"` / `"plan"` — the canonical setting label.
    pub mode_label: &'static str,
    /// Color used for the mode chip.
    pub mode_color: Color,
    /// Color used for small separators between chips.
    pub text_dim_color: Color,
    /// Color used for the model label.
    pub text_hint_color: Color,
    /// Color used for steady secondary chips such as cost.
    pub text_muted_color: Color,
    /// Background color for the full footer/status bar row.
    pub footer_bg: Color,
    /// Status label like `"ready"`, `"thinking ⌫"`, `"working"`. When the
    /// label equals `"ready"` the footer hides the status segment entirely.
    pub state_label: String,
    /// Color used for the status label.
    pub state_color: Color,
    /// Coherence chip spans (empty when no active intervention).
    pub coherence: Vec<Span<'static>>,
    /// Sub-agent count chip spans (empty when zero in-flight).
    pub agents: Vec<Span<'static>>,
    /// Reasoning-replay chip spans (empty when zero / not applicable).
    pub reasoning_replay: Vec<Span<'static>>,
    /// Cache-hit-rate chip spans (empty when no usage reported).
    pub cache: Vec<Span<'static>>,
    /// MCP server health chip spans (empty when no MCP servers configured).
    /// Populated lazily — see [`footer_mcp_chip`]. (#502)
    pub mcp: Vec<Span<'static>>,
    /// Cumulative model-work chip spans ("worked 3h 12m"). Sums the
    /// elapsed time of completed turns (from `App::cumulative_turn_duration`),
    /// **not** wall-clock since launch — an idle TUI shouldn't claim
    /// it's been "working." Empty until cumulative turn time crosses
    /// 60s. Populated by [`footer_worked_chip`]. (#448)
    pub worked: Vec<Span<'static>>,
    /// Snapshot of the global retry-status surface (#499). Sampled once
    /// at props-build time and rendered as a foreground banner on the
    /// left of the footer when active. Captured here (rather than read
    /// from `retry_status` at render time) so tests can pin a
    /// deterministic state without racing the parallel runner.
    pub retry: crate::retry_status::RetryState,
    /// Session-cost chip spans (empty when below the display threshold).
    /// Rendered in the left cluster (after the model name) — cost is steady
    /// info, not a transient signal, so it lives with mode and model.
    pub cost: Vec<Span<'static>>,
    /// Optional toast that, when present, replaces the left status line.
    pub toast: Option<FooterToast>,
    /// When `Some(frame_idx)`, the gap between the left status line and the
    /// right-hand chips is filled with an animated water-spout strip keyed
    /// off `frame_idx` (deterministic given the frame). `None` keeps the gap
    /// as plain whitespace, which is the idle/ready state.
    pub working_strip_frame: Option<u64>,
}

const WAVE_GLYPHS: [char; 8] = [
    '\u{2581}', // ▁
    '\u{2582}', // ▂
    '\u{2583}', // ▃
    '\u{2584}', // ▄
    '\u{2585}', // ▅
    '\u{2586}', // ▆
    '\u{2587}', // ▇
    '\u{2588}', // █
];

/// One frame of the footer's live-work wave animation. `col` is the cell
/// index inside the strip, `width` the strip's total width, `frame` the raw
/// millisecond counter. Returns the glyph that should appear in that cell on
/// that frame.
///
/// Visual: a full-width phase-shifted wave made from one-cell block-height
/// glyphs. The earlier crest-pair animation only changed when rounded crest
/// positions crossed a terminal cell boundary; at an 80 ms repaint cadence it
/// read as visible hops. Sampling a few moving sine components gives every
/// repaint a new surface while keeping the math deterministic for tests.
#[must_use]
pub fn footer_working_strip_glyph_at(col: usize, width: usize, frame: u64) -> char {
    if width == 0 {
        return ' ';
    }

    let t = frame as f64 / 1000.0;
    let x = col as f64;

    let primary = (x * 0.52 - t * 8.0).sin();
    let swell = (x * 0.18 + t * 3.1).sin() * 0.35;
    let shimmer = (x * 1.35 - t * 11.0).sin() * 0.12;
    let value = ((primary + swell + shimmer) / 1.47).clamp(-1.0, 1.0);
    let normalized = (value + 1.0) * 0.5;
    let idx = (normalized * (WAVE_GLYPHS.len() - 1) as f64).round() as usize;
    WAVE_GLYPHS[idx.min(WAVE_GLYPHS.len() - 1)]
}

/// Build the per-frame live-work wave string of `width` characters. Empty string
/// when width is 0. The result is the same visual width as requested (one
/// char per column for the selected block-height glyphs) and is safe to drop
/// into a `Span` between the footer's left and right segments.
#[must_use]
pub fn footer_working_strip_string(width: usize, frame: u64) -> String {
    let mut out = String::with_capacity(width * 4);
    for col in 0..width {
        out.push(footer_working_strip_glyph_at(col, width, frame));
    }
    out
}

/// Pulse the localized "working" label through 0–3 trailing ASCII dots
/// keyed off `frame`. The cycle period is 4 frames (matching the four
/// states), so adjacent ticks visibly differ. Dots stay ASCII regardless
/// of locale so the animation reads identically across scripts. Returns a
/// `String` so callers can drop it into a `Span::styled` without lifetime
/// gymnastics.
#[must_use]
pub fn footer_working_label(frame: u64, locale: Locale) -> String {
    let dots = (frame % 4) as usize;
    let base = tr(locale, MessageId::FooterWorking);
    let mut out = String::with_capacity(base.len() + dots);
    out.push_str(base);
    for _ in 0..dots {
        out.push('.');
    }
    out
}

/// Build a "N agents" chip span list when there are sub-agents in flight.
/// Empty list when N == 0 hides the chip entirely. Singular for N == 1
/// reads naturally; plural otherwise. The pluralization template lives in
/// the locale registry so CJK locales can render the count without the
/// English plural-`s` artefact.
#[must_use]
pub fn footer_agents_chip(running: usize, locale: Locale) -> Vec<Span<'static>> {
    if running == 0 {
        return Vec::new();
    }
    let text = if running == 1 {
        tr(locale, MessageId::FooterAgentSingular).to_string()
    } else {
        tr(locale, MessageId::FooterAgentsPlural).replace("{count}", &running.to_string())
    };
    vec![Span::styled(
        text,
        Style::default().fg(palette::DEEPSEEK_SKY),
    )]
}

/// Build the cumulative-elapsed chip ("worked 3h 12m") for the
/// footer's right cluster (#448). Hidden during the first minute of
/// a session so a fresh launch doesn't render a noisy `worked 5s`
/// indicator that immediately starts ticking. Above the threshold,
/// reuses [`crate::tui::notifications::humanize_duration`] for
/// consistent w/d/h/m formatting.
#[must_use]
pub fn footer_worked_chip(elapsed: std::time::Duration) -> Vec<Span<'static>> {
    if elapsed < std::time::Duration::from_secs(60) {
        return Vec::new();
    }
    let label = format!(
        "worked {}",
        crate::tui::notifications::humanize_duration(elapsed)
    );
    vec![Span::styled(
        label,
        Style::default().fg(palette::TEXT_MUTED),
    )]
}

/// Build the "MCP M/N" health chip (#502) from the user's stored
/// snapshot. `connected` is the number of servers currently reachable;
/// `configured` is the number declared in the user's MCP config. When
/// `configured` is zero the chip is hidden entirely.
///
/// Colour-codes the count by health:
/// - all reachable → success
/// - some reachable → warning
/// - none reachable but at least one configured → error
/// - configured but no live snapshot yet → muted (count only)
#[must_use]
pub fn footer_mcp_chip(connected: Option<usize>, configured: usize) -> Vec<Span<'static>> {
    if configured == 0 {
        return Vec::new();
    }
    let (label, color) = match connected {
        None => (format!("MCP {configured}"), palette::TEXT_MUTED),
        Some(c) if c == configured => (format!("MCP {c}/{configured}"), palette::STATUS_SUCCESS),
        Some(0) => (format!("MCP 0/{configured}"), palette::STATUS_ERROR),
        Some(c) => (format!("MCP {c}/{configured}"), palette::STATUS_WARNING),
    };
    vec![Span::styled(label, Style::default().fg(color))]
}

/// A status toast routed to the footer's left segment for a short time.
#[derive(Debug, Clone)]
pub struct FooterToast {
    pub text: String,
    pub color: Color,
}

impl FooterProps {
    /// Build footer props from common app state. Helpers in `tui/ui.rs`
    /// (e.g. `footer_state_label`, `footer_coherence_spans`) supply the
    /// pre-styled spans and labels — this constructor just bundles them.
    ///
    /// Argument fan-out is intentional: each input maps 1:1 to a piece of
    /// pre-computed footer content the caller resolved from `App`. Forcing
    /// these into a builder would obscure the call site without making the
    /// data flow any clearer.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn from_app(
        app: &App,
        toast: Option<FooterToast>,
        state_label: &'static str,
        state_color: Color,
        coherence: Vec<Span<'static>>,
        agents: Vec<Span<'static>>,
        reasoning_replay: Vec<Span<'static>>,
        cache: Vec<Span<'static>>,
        cost: Vec<Span<'static>>,
    ) -> Self {
        let (mode_label, mode_color) = mode_style(app);
        // MCP chip (#502) — passive, derived from the user's existing
        // snapshot. `connected` is `None` until the user runs `/mcp`,
        // which is the same trigger the issue spec accepts for now.
        let mcp_configured = app.mcp_configured_count;
        let mcp_connected = app
            .mcp_snapshot
            .as_ref()
            .map(|s| s.servers.iter().filter(|server| server.connected).count());
        let mcp = footer_mcp_chip(mcp_connected, mcp_configured);
        // #448: cumulative work-time chip. Sums actual turn durations
        // (set on `TurnComplete`) rather than wall-clock uptime — a TUI
        // that's been open and idle for 4 minutes shouldn't claim
        // "worked 4m". The chip stays empty until enough turns add up
        // to cross the 60s threshold inside `footer_worked_chip`.
        let worked = footer_worked_chip(app.cumulative_turn_duration);
        Self {
            model: app.model_display_label(),
            mode_label,
            mode_color,
            text_dim_color: app.ui_theme.text_dim,
            text_hint_color: app.ui_theme.text_hint,
            text_muted_color: app.ui_theme.text_muted,
            footer_bg: app.ui_theme.footer_bg,
            state_label: state_label.to_string(),
            state_color,
            coherence,
            agents,
            reasoning_replay,
            cache,
            mcp,
            worked,
            cost,
            toast,
            working_strip_frame: None,
            retry: crate::retry_status::snapshot(),
        }
    }
}

fn mode_style(app: &App) -> (&'static str, Color) {
    let label = match app.mode {
        AppMode::Agent => "agent",
        AppMode::Yolo => "yolo",
        AppMode::Plan => "plan",
    };
    let color = match app.mode {
        AppMode::Agent => app.ui_theme.mode_agent,
        AppMode::Yolo => app.ui_theme.mode_yolo,
        AppMode::Plan => app.ui_theme.mode_plan,
    };
    (label, color)
}

/// Pure-render footer. Build once per frame, then `render(area, buf)`.
pub struct FooterWidget {
    props: FooterProps,
}

impl FooterWidget {
    #[must_use]
    pub fn new(props: FooterProps) -> Self {
        Self { props }
    }

    fn auxiliary_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        // `cost` is rendered in the left cluster now — keep it out of the
        // right-hand chip parade. Coherence / agents / replay / cache are
        // transient signals; they belong on the right where they appear and
        // disappear without disturbing the steady mode·model·cost line.
        let parts: Vec<&Vec<Span<'static>>> = [
            &self.props.coherence,
            &self.props.agents,
            &self.props.reasoning_replay,
            &self.props.cache,
            &self.props.mcp,
            // `worked` is the lowest-priority chip — drops first under
            // narrow widths (the priority loop below removes from the
            // tail). `cost` is steady info and stays in the left
            // cluster where the eye finds it without scanning.
            &self.props.worked,
        ]
        .into_iter()
        .filter(|spans| !spans.is_empty())
        .collect();

        // Try to fit as many parts as possible, dropping from the end.
        for end in (0..=parts.len()).rev() {
            let mut combined: Vec<Span<'static>> = Vec::new();
            for (i, part) in parts[..end].iter().enumerate() {
                if i > 0 {
                    combined.push(Span::raw("  "));
                }
                combined.extend(part.iter().cloned());
            }
            if span_width(&combined) <= max_width {
                return combined;
            }
        }
        Vec::new()
    }

    fn toast_spans(toast: &FooterToast, max_width: usize) -> Vec<Span<'static>> {
        let truncated = truncate_to_width(&toast.text, max_width.max(1));
        vec![Span::styled(truncated, Style::default().fg(toast.color))]
    }

    /// Build the left status line with priority-ordered hint dropping.
    ///
    /// Priority order (highest to lowest — last to drop):
    /// 1. Mode label (always visible at any width; truncated only as a last resort)
    /// 2. Model name (always visible; then truncated mid-word once status & cost are gone)
    /// 3. Cost chip — drops second after status (steady-info still wants to be visible)
    /// 4. Status label (e.g. "working", "draft") — drops first when space is tight
    ///
    /// At every width ≥40 cols the line never wraps mid-hint: the widget
    /// chooses one of (`mode · model · cost · status`, `mode · model · cost`,
    /// `mode · model`, `mode`) and renders that single line within
    /// `max_width`. Cost lives between model and status so the eye finds
    /// "what's this run going to cost me" without scanning past the wave.
    fn status_line_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        if max_width == 0 {
            return Vec::new();
        }

        let mode_label = self.props.mode_label;
        let sep = " \u{00B7} ";
        let model = self.props.model.as_str();
        let show_status = self.props.state_label != "ready";
        let status_label = self.props.state_label.as_str();
        let cost_text = spans_text(&self.props.cost);
        let show_cost = !cost_text.is_empty();

        let mode_w = mode_label.width();
        let sep_w = sep.width();
        let model_w = UnicodeWidthStr::width(model);
        let status_w = status_label.width();
        let cost_w = cost_text.width();

        // Tier 1: mode · model · cost · status — everything fits.
        let full_w = mode_w
            + sep_w
            + model_w
            + if show_cost { sep_w + cost_w } else { 0 }
            + if show_status { sep_w + status_w } else { 0 };
        if (show_cost || show_status) && full_w <= max_width {
            return self.build_status_line_spans(
                mode_label,
                model.to_string(),
                show_cost.then(|| cost_text.clone()),
                show_status.then_some(status_label),
            );
        }

        // Tier 2: mode · model · cost — drop status first.
        if show_cost {
            let with_cost_w = mode_w + sep_w + model_w + sep_w + cost_w;
            if with_cost_w <= max_width {
                return self.build_status_line_spans(
                    mode_label,
                    model.to_string(),
                    Some(cost_text.clone()),
                    None,
                );
            }
        }

        // Tier 3: mode · model — drop cost too.
        let mode_model_w = mode_w + sep_w + model_w;
        if mode_model_w <= max_width {
            return self.build_status_line_spans(mode_label, model.to_string(), None, None);
        }

        // Tier 4: mode · <truncated model> — keep both labels visible by
        // ellipsizing the model name. Only do this when there is enough room
        // for at least the ellipsis ("..."). Below that we drop to mode-only.
        let prefix_w = mode_w + sep_w;
        if prefix_w < max_width {
            let model_budget = max_width - prefix_w;
            if model_budget >= 4 {
                let truncated = truncate_to_width(model, model_budget);
                if !truncated.is_empty() {
                    return self.build_status_line_spans(mode_label, truncated, None, None);
                }
            }
        }

        // Tier 5: mode-only. If even the mode label cannot fit, truncate it
        // so the footer never wraps to a second row.
        if mode_w <= max_width {
            return vec![Span::styled(
                mode_label.to_string(),
                Style::default().fg(self.props.mode_color),
            )];
        }
        vec![Span::styled(
            truncate_to_width(mode_label, max_width),
            Style::default().fg(self.props.mode_color),
        )]
    }

    fn build_status_line_spans(
        &self,
        mode_label: &'static str,
        model_label: String,
        cost: Option<String>,
        status: Option<&str>,
    ) -> Vec<Span<'static>> {
        let sep = " \u{00B7} ";
        let mut spans: Vec<Span<'static>> = Vec::new();
        // Skip the mode chip when the user has toggled it off via
        // `/statusline`. The widget no longer assumes mode is always
        // present so an opt-out user doesn't see a stray separator.
        if !mode_label.is_empty() {
            spans.push(Span::styled(
                mode_label.to_string(),
                Style::default().fg(self.props.mode_color),
            ));
        }
        // Same treatment for the model label — gating both keeps the bar
        // visually tidy when only auxiliary chips remain.
        if !model_label.is_empty() {
            if !spans.is_empty() {
                spans.push(Span::styled(
                    sep.to_string(),
                    Style::default().fg(self.props.text_dim_color),
                ));
            }
            spans.push(Span::styled(
                model_label,
                Style::default().fg(self.props.text_hint_color),
            ));
        }
        if let Some(cost_text) = cost {
            if !spans.is_empty() {
                spans.push(Span::styled(
                    sep.to_string(),
                    Style::default().fg(self.props.text_dim_color),
                ));
            }
            spans.push(Span::styled(
                cost_text,
                Style::default().fg(self.props.text_muted_color),
            ));
        }
        if let Some(status_label) = status {
            if !spans.is_empty() {
                spans.push(Span::styled(
                    sep.to_string(),
                    Style::default().fg(self.props.text_dim_color),
                ));
            }
            spans.push(Span::styled(
                status_label.to_string(),
                Style::default().fg(self.props.state_color),
            ));
        }
        spans
    }

    fn left_spans(&self, max_width: usize) -> Vec<Span<'static>> {
        if let Some(banner) = retry_banner_spans(max_width, &self.props) {
            // Retry banner takes precedence over toast and the regular
            // status line so the user sees it loud and clear (#499).
            // The banner clears automatically on success or on the next
            // `TurnStarted` (engine emits the clear).
            banner
        } else if let Some(toast) = self.props.toast.as_ref() {
            Self::toast_spans(toast, max_width)
        } else {
            self.status_line_spans(max_width)
        }
    }
}

fn spans_text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect::<String>()
}

/// Render the retry banner (#499) when the props' captured snapshot
/// reports an active retry or a final failure. Returns `None` when idle
/// so callers fall back to the regular status line / toast.
fn retry_banner_spans(max_width: usize, props: &FooterProps) -> Option<Vec<Span<'static>>> {
    let (label, color) = match &props.retry {
        crate::retry_status::RetryState::Active(banner) => {
            let secs = props.retry.seconds_remaining().unwrap_or(0);
            // Round to 1s — we redraw each frame anyway so the
            // countdown ticks visually without us having to schedule
            // anything extra.
            (
                format!("⟳ retry {} in {secs}s — {}", banner.attempt, banner.reason),
                crate::palette::STATUS_WARNING,
            )
        }
        crate::retry_status::RetryState::Failed { reason, .. } => {
            (format!("× failed: {reason}"), crate::palette::STATUS_ERROR)
        }
        crate::retry_status::RetryState::Idle => return None,
    };
    let truncated = truncate_to_width(&label, max_width);
    Some(vec![Span::styled(truncated, Style::default().fg(color))])
}

impl Renderable for FooterWidget {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        let available_width = area.width as usize;
        if available_width == 0 {
            return;
        }

        let preview_left_spans = self.left_spans(available_width);
        let preview_left_width = span_width(&preview_left_spans);
        let right_budget = available_width
            .saturating_sub(preview_left_width)
            .saturating_sub(2);
        let right_spans = self.auxiliary_spans(right_budget);
        let right_width = span_width(&right_spans);
        let min_gap = if right_width > 0 { 2 } else { 0 };
        let max_left_width = available_width
            .saturating_sub(right_width)
            .saturating_sub(min_gap)
            .max(1);
        let left_spans = self.left_spans(max_left_width);

        let left_width = span_width(&left_spans);
        let spacer_width = available_width.saturating_sub(left_width + right_width);

        // When a turn is in flight, fill the gap with a thin animated water-
        // spout strip; otherwise the gap stays as plain whitespace.
        let spacer_span = match self.props.working_strip_frame {
            Some(frame) if spacer_width > 0 => Span::styled(
                footer_working_strip_string(spacer_width, frame),
                Style::default().fg(palette::DEEPSEEK_SKY),
            ),
            _ => Span::raw(" ".repeat(spacer_width)),
        };

        let mut all_spans = left_spans;
        all_spans.push(spacer_span);
        all_spans.extend(right_spans);

        let paragraph =
            Paragraph::new(Line::from(all_spans)).style(Style::default().bg(self.props.footer_bg));
        paragraph.render(area, buf);
    }

    fn desired_height(&self, _width: u16) -> u16 {
        1
    }
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        return text.chars().take(max_width).collect();
    }

    let mut out = String::new();
    let mut width = 0usize;
    let limit = max_width.saturating_sub(3);
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > limit {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::{FooterProps, FooterWidget, Renderable};
    use crate::config::Config;
    use crate::localization::Locale;
    use crate::palette;
    use crate::tui::app::{App, AppMode, TuiOptions};
    use ratatui::{
        style::{Color, Style},
        text::Span,
    };
    use std::path::PathBuf;
    use unicode_width::UnicodeWidthStr;

    fn make_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-flash".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        // App::new may pick up `default_model` from a local user Settings
        // file, which overrides the option above. Pin the model explicitly
        // so these tests are independent of any host-side configuration.
        app.model = "deepseek-v4-flash".to_string();
        app
    }

    fn idle_props_for(app: &App) -> FooterProps {
        let mut props = FooterProps::from_app(
            app,
            None,
            "ready",
            palette::TEXT_MUTED,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
        );
        // `from_app` reads the process-wide retry-status surface; pin
        // `Idle` so footer tests don't pick up state set by retry-banner
        // tests running in parallel.
        props.retry = crate::retry_status::RetryState::Idle;
        props
    }

    #[test]
    fn from_app_idle_state_carries_ready_label_and_no_chips() {
        let app = make_app();
        let props = idle_props_for(&app);

        assert_eq!(props.state_label, "ready");
        assert_eq!(props.state_color, palette::TEXT_MUTED);
        assert_eq!(props.mode_label, "agent");
        assert_eq!(props.mode_color, palette::MODE_AGENT);
        assert_eq!(props.text_dim_color, palette::TEXT_DIM);
        assert_eq!(props.text_hint_color, palette::TEXT_HINT);
        assert_eq!(props.text_muted_color, palette::TEXT_MUTED);
        assert_eq!(props.model, "deepseek-v4-flash");
        assert!(props.coherence.is_empty());
        assert!(props.agents.is_empty());
        assert!(props.cache.is_empty());
        assert!(props.cost.is_empty());
        assert!(props.reasoning_replay.is_empty());
        // #448: fresh apps don't get a `worked` chip until completed
        // turns have added up to >= 60s of model work. A freshly-built
        // App has cumulative_turn_duration == 0 so the chip is empty.
        assert!(props.worked.is_empty());
        assert!(props.toast.is_none());
    }

    #[test]
    fn worked_chip_tracks_completed_turn_time_not_session_uptime() {
        // Regression test for the v0.8.8 takedown: the chip used to
        // read `App::session_started_at.elapsed()`, so a TUI that had
        // been open and idle for several minutes claimed "worked 3m"
        // even though no turn had ever fired. The chip now sources
        // from `App::cumulative_turn_duration`, which is only ever
        // incremented on `TurnComplete`. Pin both directions:
        //
        //   1. cumulative == 0 (no turn finished yet)  → empty
        //   2. cumulative crosses 60s (real work)      → label shows
        //   3. wall-clock since launch is irrelevant   → not consulted
        let mut app = make_app();
        // The whole point: cumulative_turn_duration starts at zero,
        // so however long the TUI has been open the chip stays empty
        // until a turn actually completes and adds time.
        let props = idle_props_for(&app);
        assert!(
            props.worked.is_empty(),
            "idle app with zero cumulative turn time must not show worked chip"
        );

        // A real turn finishes for 90s of model work — chip lights up.
        // (`humanize_duration` keeps both units when both are non-zero,
        // so 90s renders as `1m 30s`, not `1m`.)
        app.cumulative_turn_duration = std::time::Duration::from_secs(90);
        let props = idle_props_for(&app);
        let text: String = props
            .worked
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert_eq!(text, "worked 1m 30s");
    }

    #[test]
    fn footer_worked_chip_hidden_below_one_minute() {
        use std::time::Duration;
        for secs in [0, 1, 30, 59] {
            let chip = super::footer_worked_chip(Duration::from_secs(secs));
            assert!(
                chip.is_empty(),
                "worked chip must be hidden at {secs}s; got {chip:?}"
            );
        }
    }

    #[test]
    fn footer_worked_chip_shows_humanized_label_above_threshold() {
        use std::time::Duration;
        // 1 minute on the dot — boundary, must render.
        let chip = super::footer_worked_chip(Duration::from_secs(60));
        let text: String = chip.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "worked 1m");

        // 3h 12m — the issue's golden example.
        let chip = super::footer_worked_chip(Duration::from_secs(11_550));
        let text: String = chip.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "worked 3h 12m");

        // Multi-day session — exercises the d/h band.
        let chip = super::footer_worked_chip(Duration::from_secs(2 * 86_400 + 5 * 3600));
        let text: String = chip.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "worked 2d 5h");
    }

    #[test]
    fn from_app_loading_state_uses_thinking_label_and_warning_color() {
        let app = make_app();
        let props = FooterProps::from_app(
            &app,
            None,
            "thinking \u{238B}",
            palette::STATUS_WARNING,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
        );

        assert!(props.state_label.starts_with("thinking"));
        assert_eq!(props.state_color, palette::STATUS_WARNING);
    }

    #[test]
    fn from_app_statusline_colors_come_from_ui_theme() {
        let mut app = make_app();
        app.ui_theme.mode_agent = Color::Rgb(1, 2, 3);
        app.ui_theme.text_dim = Color::Rgb(4, 5, 6);
        app.ui_theme.text_hint = Color::Rgb(7, 8, 9);
        app.ui_theme.text_muted = Color::Rgb(10, 11, 12);
        app.ui_theme.footer_bg = Color::Rgb(13, 14, 15);

        let props = idle_props_for(&app);

        assert_eq!(props.mode_color, Color::Rgb(1, 2, 3));
        assert_eq!(props.text_dim_color, Color::Rgb(4, 5, 6));
        assert_eq!(props.text_hint_color, Color::Rgb(7, 8, 9));
        assert_eq!(props.text_muted_color, Color::Rgb(10, 11, 12));
        assert_eq!(props.footer_bg, Color::Rgb(13, 14, 15));
    }

    #[test]
    fn render_applies_footer_background_to_full_row() {
        let mut app = make_app();
        app.ui_theme.footer_bg = Color::Rgb(13, 14, 15);
        let props = idle_props_for(&app);
        let widget = FooterWidget::new(props);
        let area = ratatui::layout::Rect::new(0, 0, 60, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);

        widget.render(area, &mut buf);

        for x in 0..area.width {
            assert_eq!(buf[(x, 0)].bg, Color::Rgb(13, 14, 15));
        }
    }

    // ---- agents chip wording ----
    #[test]
    fn footer_agents_chip_is_empty_when_no_agents_running() {
        let chip = super::footer_agents_chip(0, Locale::En);
        assert!(chip.is_empty(), "0 agents in flight → no chip");
    }

    #[test]
    fn footer_agents_chip_uses_singular_for_one() {
        let chip = super::footer_agents_chip(1, Locale::En);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].content.as_ref(), "1 agent");
    }

    #[test]
    fn footer_agents_chip_uses_plural_for_many() {
        let chip = super::footer_agents_chip(3, Locale::En);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].content.as_ref(), "3 agents");
    }

    #[test]
    fn footer_agents_chip_renders_into_widget() {
        let app = make_app();
        let agents = super::footer_agents_chip(2, Locale::En);
        let props = FooterProps::from_app(
            &app,
            None,
            "ready",
            palette::TEXT_MUTED,
            Vec::<Span<'static>>::new(),
            agents,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
        );
        let widget = FooterWidget::new(props);
        let area = ratatui::layout::Rect::new(0, 0, 60, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            rendered.contains("2 agents"),
            "expected agents chip in render: {rendered:?}",
        );
    }

    #[test]
    fn from_app_mode_color_matches_mode_for_each_variant() {
        let mut app = make_app();
        let cases = [
            (AppMode::Agent, "agent", palette::MODE_AGENT),
            (AppMode::Yolo, "yolo", palette::MODE_YOLO),
            (AppMode::Plan, "plan", palette::MODE_PLAN),
        ];
        for (mode, expected_label, expected_color) in cases {
            app.mode = mode;
            let props = idle_props_for(&app);
            assert_eq!(
                props.mode_label, expected_label,
                "label mismatch for {mode:?}",
            );
            assert_eq!(
                props.mode_color, expected_color,
                "color mismatch for {mode:?}",
            );
        }
    }

    #[test]
    fn footer_mcp_chip_hidden_when_no_servers() {
        assert!(super::footer_mcp_chip(None, 0).is_empty());
        assert!(super::footer_mcp_chip(Some(0), 0).is_empty());
    }

    #[test]
    fn footer_mcp_chip_shows_count_only_until_snapshot_arrives() {
        let spans = super::footer_mcp_chip(None, 3);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "MCP 3");
    }

    #[test]
    fn footer_mcp_chip_uses_success_color_when_all_connected() {
        let spans = super::footer_mcp_chip(Some(3), 3);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "MCP 3/3");
        assert_eq!(spans[0].style.fg, Some(palette::STATUS_SUCCESS));
    }

    #[test]
    fn footer_mcp_chip_uses_warning_color_when_partial() {
        let spans = super::footer_mcp_chip(Some(2), 3);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "MCP 2/3");
        assert_eq!(spans[0].style.fg, Some(palette::STATUS_WARNING));
    }

    #[test]
    fn footer_mcp_chip_uses_error_color_when_zero_connected() {
        let spans = super::footer_mcp_chip(Some(0), 3);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "MCP 0/3");
        assert_eq!(spans[0].style.fg, Some(palette::STATUS_ERROR));
    }

    #[test]
    fn render_shows_retry_banner_when_active() {
        // Since `FooterProps::retry` is now a captured snapshot rather
        // than a global read at render time, we can pin the state on
        // the props directly without touching the global surface.
        let app = make_app();
        let mut props = idle_props_for(&app);
        props.retry = crate::retry_status::RetryState::Active(crate::retry_status::RetryBanner {
            attempt: 2,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(7),
            reason: "rate limited".to_string(),
        });
        let widget = FooterWidget::new(props);
        let area = ratatui::layout::Rect::new(0, 0, 80, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            rendered.contains("retry 2"),
            "expected retry banner in render: {rendered:?}",
        );
        assert!(
            rendered.contains("rate limited"),
            "expected reason in render: {rendered:?}",
        );
    }

    #[test]
    fn render_shows_failure_row_when_failed() {
        let app = make_app();
        let mut props = idle_props_for(&app);
        props.retry = crate::retry_status::RetryState::Failed {
            reason: "upstream 500".to_string(),
            since: std::time::Instant::now(),
        };
        let widget = FooterWidget::new(props);
        let area = ratatui::layout::Rect::new(0, 0, 80, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        widget.render(area, &mut buf);
        let rendered: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(
            rendered.contains("failed"),
            "expected failure row: {rendered:?}",
        );
        assert!(
            rendered.contains("upstream 500"),
            "expected reason: {rendered:?}",
        );
    }

    #[test]
    fn render_emits_mode_and_model_when_idle() {
        let app = make_app();
        let props = idle_props_for(&app);
        let widget = FooterWidget::new(props);

        let area = ratatui::layout::Rect::new(0, 0, 60, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        widget.render(area, &mut buf);

        let rendered: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(rendered.contains("agent"));
        assert!(rendered.contains("deepseek-v4-flash"));
        assert!(!rendered.contains("ready"));
    }

    #[test]
    fn working_strip_string_width_matches_request() {
        // The strip must produce exactly `width` characters per frame —
        // otherwise the spacer math in `FooterWidget::render` would
        // mis-align the right-hand chips. Each wave glyph is one cell wide.
        for width in [0usize, 1, 8, 60, 200] {
            let s = super::footer_working_strip_string(width, 7);
            assert_eq!(s.chars().count(), width, "width {width} mismatch");
        }
    }

    #[test]
    fn working_strip_glyph_is_deterministic_per_frame() {
        // Same (col, width, frame) -> same glyph. Frames are raw
        // milliseconds so the strip can move at repaint cadence.
        let a = super::footer_working_strip_string(40, 150);
        let b = super::footer_working_strip_string(40, 150);
        assert_eq!(a, b, "deterministic given the same frame");
        let c = super::footer_working_strip_string(40, 230);
        assert_ne!(a, c, "advancing one repaint window must change the strip",);
    }

    #[test]
    fn working_strip_renders_glyphs_only_when_frame_is_some() {
        // Idle: spacer is plain whitespace. Active: spacer contains the
        // wave animation glyphs and visibly differs from the idle render.
        let app = make_app();
        let mut props = idle_props_for(&app);

        let area = ratatui::layout::Rect::new(0, 0, 80, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        FooterWidget::new(props.clone()).render(area, &mut buf);
        let idle: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();

        props.working_strip_frame = Some(600);
        let mut buf2 = ratatui::buffer::Buffer::empty(area);
        FooterWidget::new(props).render(area, &mut buf2);
        let active: String = (0..area.width).map(|x| buf2[(x, 0)].symbol()).collect();

        assert_ne!(
            idle, active,
            "active footer must visibly differ from idle one"
        );
        assert!(
            active
                .chars()
                .any(|glyph| super::WAVE_GLYPHS.contains(&glyph)),
            "active strip must contain at least one animation glyph: {active:?}",
        );
    }

    #[test]
    fn working_strip_changes_at_repaint_cadence() {
        let width = 60;
        let f0 = super::footer_working_strip_string(width, 0);
        let f80 = super::footer_working_strip_string(width, 80);
        let changed = f0
            .chars()
            .zip(f80.chars())
            .filter(|(before, after)| before != after)
            .count();
        assert!(
            changed > width / 4,
            "expected the wave to drift across one 80ms repaint; changed {changed}/{width}"
        );
    }

    #[test]
    fn working_strip_renders_multiple_wave_heights() {
        let s = super::footer_working_strip_string(60, 0);
        let mut distinct = Vec::new();
        for glyph in s.chars() {
            if super::WAVE_GLYPHS.contains(&glyph) && !distinct.contains(&glyph) {
                distinct.push(glyph);
            }
        }
        assert!(
            distinct.len() >= 5,
            "expected several wave heights, saw {distinct:?}",
        );
    }

    #[test]
    fn working_label_pulses_dots_through_full_cycle() {
        // The label sequence `working` → `working.` → `working..` →
        // `working...` then wraps back. Each frame is a discrete tick;
        // the cycle is exactly 4 frames so adjacent ticks visibly differ.
        assert_eq!(super::footer_working_label(0, Locale::En), "working");
        assert_eq!(super::footer_working_label(1, Locale::En), "working.");
        assert_eq!(super::footer_working_label(2, Locale::En), "working..");
        assert_eq!(super::footer_working_label(3, Locale::En), "working...");
        assert_eq!(
            super::footer_working_label(4, Locale::En),
            "working",
            "wraps back at frame 4",
        );
        assert_eq!(super::footer_working_label(7, Locale::En), "working...");
    }

    /// Render the footer at `width` and return the visible single-line text.
    fn render_at_width(props: FooterProps, width: u16) -> String {
        let area = ratatui::layout::Rect::new(0, 0, width, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        FooterWidget::new(props).render(area, &mut buf);
        (0..area.width)
            .map(|x| buf[(x, 0)].symbol())
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    fn props_with_status(state: &str) -> FooterProps {
        let app = make_app();
        FooterProps::from_app(
            &app,
            None,
            // Production state labels are `&'static str`; for tests we leak a
            // copy to match that lifetime.
            Box::leak(state.to_string().into_boxed_str()),
            palette::DEEPSEEK_SKY,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
        )
    }

    /// Issue #88 — at the widest tier the footer shows mode · model · status
    /// without any truncation.
    #[test]
    fn footer_priority_drop_full_at_120_cols() {
        let props = props_with_status("working");
        let line = render_at_width(props, 120);
        assert!(line.contains("agent"), "mode visible: {line:?}");
        assert!(
            line.contains("deepseek-v4-flash"),
            "model visible: {line:?}"
        );
        assert!(line.contains("working"), "status visible: {line:?}");
        assert!(!line.contains("..."), "no truncation expected: {line:?}");
    }

    #[test]
    fn footer_priority_drop_full_at_100_cols() {
        let props = props_with_status("working");
        let line = render_at_width(props, 100);
        assert!(line.contains("agent"));
        assert!(line.contains("deepseek-v4-flash"));
        assert!(line.contains("working"));
    }

    /// At 80 cols the short status label "working" still fits alongside mode +
    /// model. The line never wraps mid-hint.
    #[test]
    fn footer_priority_drop_full_at_80_cols() {
        let props = props_with_status("working");
        let line = render_at_width(props, 80);
        assert!(line.contains("agent"));
        assert!(line.contains("deepseek-v4-flash"));
        assert!(!line.contains("..."), "no mid-word truncation: {line:?}");
        assert!(line.len() <= 80, "fits in 80 cols: {line:?}");
    }

    /// Status drops before the model is truncated. With a longer status label
    /// at 40 cols the status segment is dropped to keep mode + model intact.
    #[test]
    fn footer_priority_drop_status_first_at_40_cols() {
        let props = props_with_status("refreshing context");
        // "agent · deepseek-v4-flash · refreshing context" = 46 cols. At 40
        // the status label drops, keeping mode + model verbatim.
        let line = render_at_width(props, 40);
        assert!(line.contains("agent"), "mode kept: {line:?}");
        assert!(
            line.contains("deepseek-v4-flash"),
            "model kept verbatim: {line:?}"
        );
        assert!(
            !line.contains("refreshing"),
            "status dropped before model truncated: {line:?}",
        );
        assert!(line.len() <= 40, "fits in 40 cols: {line:?}");
    }

    /// At 60 cols mode + model + a long status all just fit (49 cols), so the
    /// whole line is preserved.
    #[test]
    fn footer_priority_drop_full_at_60_cols() {
        let props = props_with_status("working");
        let line = render_at_width(props, 60);
        assert!(line.contains("agent"));
        assert!(line.contains("deepseek-v4-flash"));
        assert!(line.contains("working"));
    }

    /// Below 30 cols the model truncates with an ellipsis only after the
    /// status label has already been dropped. Mode label always survives.
    #[test]
    fn footer_priority_drop_truncates_model_only_when_status_already_gone() {
        let props = props_with_status("working");
        let line = render_at_width(props, 20);
        assert!(line.starts_with("agent"), "mode stays at front: {line:?}");
        assert!(
            line.contains("..."),
            "model truncated as last resort: {line:?}"
        );
        assert!(!line.contains("working"), "status dropped: {line:?}");
    }

    fn props_with_status_and_cost(state: &str, cost: &str) -> FooterProps {
        let app = make_app();
        FooterProps::from_app(
            &app,
            None,
            Box::leak(state.to_string().into_boxed_str()),
            palette::DEEPSEEK_SKY,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            vec![Span::styled(cost.to_string(), Style::default())],
        )
    }

    #[test]
    fn render_drops_oversized_right_chips_before_they_crowd_left_status() {
        let app = make_app();
        let long_cache = vec![Span::styled(
            "Cache: 75.0% hit | hit 36000 | miss 12000".to_string(),
            Style::default(),
        )];
        let props = FooterProps::from_app(
            &app,
            None,
            "ready",
            palette::TEXT_MUTED,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            long_cache,
            Vec::<Span<'static>>::new(),
        );

        let line = render_at_width(props, 40);

        assert!(
            line.contains("agent"),
            "left status should survive: {line:?}"
        );
        assert!(
            !line.contains("Cache:"),
            "oversized right chip should drop instead of crowding the row: {line:?}",
        );
        assert!(line.width() <= 40, "footer must fit in one row: {line:?}");
    }

    #[test]
    fn render_keeps_right_chips_when_left_status_leaves_room() {
        let app = make_app();
        let cache = vec![Span::styled(
            "Cache: 75.0% hit".to_string(),
            Style::default(),
        )];
        let props = FooterProps::from_app(
            &app,
            None,
            "ready",
            palette::TEXT_MUTED,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            cache,
            Vec::<Span<'static>>::new(),
        );

        let line = render_at_width(props, 80);

        assert!(
            line.contains("agent"),
            "left status should render: {line:?}"
        );
        assert!(
            line.contains("Cache: 75.0% hit"),
            "right chip should render: {line:?}"
        );
        assert!(line.width() <= 80, "footer must fit in one row: {line:?}");
    }

    /// v0.6.6 redesign — cost lives on the LEFT, between model and status.
    /// At wide widths the line reads `mode · model · cost · status`.
    #[test]
    fn footer_cost_renders_in_left_cluster_at_wide_widths() {
        let props = props_with_status_and_cost("working", "$0.42");
        let line = render_at_width(props, 120);
        let mode_pos = line.find("agent").expect("mode visible");
        let model_pos = line.find("deepseek-v4-flash").expect("model visible");
        let cost_pos = line.find("$0.42").expect("cost visible on left");
        let status_pos = line.find("working").expect("status visible");
        assert!(mode_pos < model_pos);
        assert!(model_pos < cost_pos, "cost must follow model: {line:?}");
        assert!(cost_pos < status_pos, "cost must precede status: {line:?}");
    }

    /// Cost is preserved when status drops — cost is steady info, status is
    /// a transient signal.
    #[test]
    fn footer_cost_outranks_status_when_space_tight() {
        // "agent · deepseek-v4-flash · $0.42 · refreshing context" = 53 cols.
        // At 47 the status drops but the cost survives (47 ≥ 36 mode+model+cost).
        let props = props_with_status_and_cost("refreshing context", "$0.42");
        let line = render_at_width(props, 47);
        assert!(line.contains("agent"));
        assert!(line.contains("deepseek-v4-flash"));
        assert!(
            line.contains("$0.42"),
            "cost survives status drop: {line:?}"
        );
        assert!(!line.contains("refreshing"), "status dropped: {line:?}");
    }

    #[test]
    fn render_swaps_toast_for_status_line() {
        let app = make_app();
        let toast = super::FooterToast {
            text: "session saved".to_string(),
            color: Color::Green,
        };
        let props = FooterProps::from_app(
            &app,
            Some(toast),
            "ready",
            palette::TEXT_MUTED,
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
            Vec::<Span<'static>>::new(),
        );
        let widget = FooterWidget::new(props);

        let area = ratatui::layout::Rect::new(0, 0, 60, 1);
        let mut buf = ratatui::buffer::Buffer::empty(area);
        widget.render(area, &mut buf);

        let rendered: String = (0..area.width).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(rendered.contains("session saved"));
        assert!(!rendered.contains("agent"));
        assert!(!rendered.contains("deepseek-v4-flash"));
    }
}
