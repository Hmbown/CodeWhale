//! Session metrics strip: the compact `turns · steps │ LLM · tools │ TTFT ·
//! tok/s │ cache │ in` ledger painted on the phase strip.
//!
//! Every number here is sourced from runtime evidence the engine already
//! emits — never from transcript timestamps or estimates:
//!
//! - **turns**: `Event::TurnStarted` count (`App::turn_counter`).
//! - **steps**: model calls (`Event::TurnUsage`) plus tool calls
//!   (`Event::ToolCallComplete`) — the agent's step count.
//! - **LLM**: sum of model-call wall time. Uses `TurnUsage::request_ms`
//!   (dispatch → usage receipt) when the engine measured dispatch, else the
//!   stream duration it always reports.
//! - **tools**: sum of tool wall time from `ToolCallStarted` → `ToolCallComplete`
//!   by tool id (the runtime's own clock, taken when the events drain).
//! - **TTFT avg**: mean of `TurnUsage::first_token_ms` over the model calls
//!   that reported one.
//! - **tok/s**: provider-reported output tokens over the streamed seconds of
//!   the same calls (`duration_ms`); calls without a stream duration are
//!   excluded from both sides.
//! - **cache**: provider-reported prompt-cache hit tokens over hit + miss
//!   (`SessionState::total_cache_hit_tokens` / `total_cache_miss_tokens`).
//! - **in**: provider-reported input tokens (`SessionState::total_input_tokens`).
//!
//! When a provider never reports a metric, or its evidence has not arrived
//! yet, the cell is omitted. Nothing here is estimated or captioned.
//!
//! The strip is one row wide and never grows the layout: it lives in the
//! phase-strip ledger tail (`crate::tui::phase_strip`), between the phase
//! marker and the right-hand key hints, and drops its lowest-value groups
//! until it fits the columns that are genuinely available.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::localization::{Locale, MessageId, tr};

/// Runtime accumulators behind the strip. Lives on [`crate::tui::app::App`],
/// resets with the token breakdown when a session is loaded, so the numbers
/// describe this runtime session — the same scope as the token ledger.
#[derive(Debug, Clone, Default)]
pub struct SessionMetrics {
    /// Model calls that reported usage (`Event::TurnUsage`).
    pub model_calls: u64,
    /// Tool calls that completed (`Event::ToolCallComplete`).
    pub tool_calls: u64,
    /// Sum of model-call wall time.
    pub llm_time: Duration,
    /// Sum of tool wall time.
    pub tool_time: Duration,
    /// Sum of reported time-to-first-token values.
    ttft_total: Duration,
    /// How many model calls reported a time-to-first-token.
    ttft_samples: u64,
    /// Output tokens from calls that also reported a stream duration.
    rate_output_tokens: u64,
    /// Stream time from the same calls.
    rate_stream_time: Duration,
    /// Tools currently running, keyed by tool id, with the instant their
    /// start event drained.
    tool_started: HashMap<String, Instant>,
}

impl SessionMetrics {
    /// Fold one model-call usage receipt into the accumulators.
    pub fn record_model_call(
        &mut self,
        output_tokens: u32,
        stream_ms: u64,
        first_token_ms: Option<u64>,
        request_ms: Option<u64>,
    ) {
        self.model_calls = self.model_calls.saturating_add(1);
        let call_ms = request_ms.unwrap_or(stream_ms);
        self.llm_time = self.llm_time.saturating_add(Duration::from_millis(call_ms));
        if let Some(ttft) = first_token_ms {
            self.ttft_total = self.ttft_total.saturating_add(Duration::from_millis(ttft));
            self.ttft_samples = self.ttft_samples.saturating_add(1);
        }
        if stream_ms > 0 {
            self.rate_output_tokens = self
                .rate_output_tokens
                .saturating_add(u64::from(output_tokens));
            self.rate_stream_time = self
                .rate_stream_time
                .saturating_add(Duration::from_millis(stream_ms));
        }
    }

    /// Note that a tool started; the matching completion closes the timer.
    pub fn record_tool_started(&mut self, tool_id: &str) {
        self.record_tool_started_at(tool_id, Instant::now());
    }

    fn record_tool_started_at(&mut self, tool_id: &str, at: Instant) {
        self.tool_started.insert(tool_id.to_string(), at);
    }

    /// Note that a tool completed. Counts the call even when its start was
    /// never seen (a replayed or foreign completion), but only accrues time
    /// when the runtime saw both edges.
    pub fn record_tool_completed(&mut self, tool_id: &str) {
        self.record_tool_completed_at(tool_id, Instant::now());
    }

    fn record_tool_completed_at(&mut self, tool_id: &str, at: Instant) {
        self.tool_calls = self.tool_calls.saturating_add(1);
        if let Some(started) = self.tool_started.remove(tool_id) {
            self.tool_time = self
                .tool_time
                .saturating_add(at.saturating_duration_since(started));
        }
    }

    /// Drop in-flight tool timers (turn interrupted or failed): a tool that
    /// never completed must not leak into the next turn's accounting.
    pub fn clear_in_flight(&mut self) {
        self.tool_started.clear();
    }

    /// Model calls plus tool calls.
    #[must_use]
    pub fn steps(&self) -> u64 {
        self.model_calls.saturating_add(self.tool_calls)
    }

    /// Mean time-to-first-token, when at least one call reported it.
    #[must_use]
    pub fn ttft_average(&self) -> Option<Duration> {
        if self.ttft_samples == 0 {
            return None;
        }
        Some(self.ttft_total / u32::try_from(self.ttft_samples).unwrap_or(u32::MAX))
    }

    /// Output tokens per streamed second, when the evidence exists.
    #[must_use]
    pub fn tokens_per_second(&self) -> Option<f64> {
        let secs = self.rate_stream_time.as_secs_f64();
        if self.rate_output_tokens == 0 || !secs.is_finite() || secs <= 0.0 {
            return None;
        }
        Some(self.rate_output_tokens as f64 / secs)
    }
}

/// Everything the strip needs, decoupled from `App` so rendering can be
/// unit-tested without a full app.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MetricsSnapshot {
    pub turns: u64,
    pub steps: u64,
    pub llm_time: Duration,
    pub tool_time: Duration,
    pub ttft_avg: Option<Duration>,
    pub tokens_per_second: Option<f64>,
    /// `None` when no provider reported prompt-cache classes this session.
    pub cache_hit_percent: Option<u8>,
    pub input_tokens: u64,
}

impl MetricsSnapshot {
    /// True when there is nothing to say yet (fresh session).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns == 0 && self.steps == 0 && self.input_tokens == 0
    }
}

/// One rendered cell: a value with its localized short label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricCell {
    pub label: String,
    pub value: String,
    /// `label` first (`4 turns`) or value first (`LLM 11m46s`).
    pub value_first: bool,
}
/// Group priority, highest kept first. When the row is too narrow, groups
/// are dropped from the end of this list; inside a group the second cell
/// (steps, tools, tok/s) is dropped before the group itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricGroup {
    Input,
    Cache,
    Llm,
    Turns,
    Latency,
}

/// The DSH-style layout order, left to right.
const GROUP_ORDER: [MetricGroup; 5] = [
    MetricGroup::Turns,
    MetricGroup::Llm,
    MetricGroup::Latency,
    MetricGroup::Cache,
    MetricGroup::Input,
];

/// A group of one or two cells separated by ` · `.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricGroupCells {
    pub group: MetricGroup,
    pub cells: Vec<MetricCell>,
}

/// Separators used between cells and between groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Separators {
    pub cell: &'static str,
    pub group: &'static str,
}

impl Separators {
    /// Unicode: ` · ` inside a group, ` │ ` between groups.
    pub const UNICODE: Self = Self {
        cell: " · ",
        group: " │ ",
    };
    /// ASCII-safe: ` . ` and ` | `.
    pub const ASCII: Self = Self {
        cell: " . ",
        group: " | ",
    };

    #[must_use]
    pub fn for_ascii(ascii_safe: bool) -> Self {
        if ascii_safe {
            Self::ASCII
        } else {
            Self::UNICODE
        }
    }
}

/// Format a duration the way the strip does: `11m46s`, `1h02m`, `1.5s`, `320ms`.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let ms = duration.as_millis();
    if ms == 0 {
        return "0s".to_string();
    }
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    let secs = duration.as_secs();
    if secs < 60 {
        let tenths = (ms + 50) / 100;
        return format!("{}.{}s", tenths / 10, tenths % 10);
    }
    if secs < 3_600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3_600, (secs % 3_600) / 60)
}

/// Format a token count: `842`, `12.3K`, `9.3M`, `1.2B`.
#[must_use]
pub fn format_tokens(tokens: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1_000_000_000, "B"), (1_000_000, "M"), (1_000, "K")];
    for (scale, suffix) in UNITS {
        if tokens >= scale {
            let scaled = tokens as f64 / scale as f64;
            return if scaled >= 100.0 {
                format!("{scaled:.0}{suffix}")
            } else {
                format!("{scaled:.1}{suffix}")
            };
        }
    }
    tokens.to_string()
}

/// Format an output rate: `120` or `7.5` (the label carries `tok/s`).
#[must_use]
pub fn format_rate(rate: f64) -> String {
    if rate < 10.0 {
        format!("{rate:.1}")
    } else {
        format!("{rate:.0}")
    }
}

/// Build the cells for every group that has something truthful to show.
///
/// A cell whose evidence has not arrived is omitted — never a placeholder:
/// `TTFT avg` / `tok/s` appear only once a model call reported them, `Cache
/// hit` only when a provider reported cache classes, `Input` only after the
/// first usage receipt. Turn cells are present once the session has started
/// (zero turns is a real count). Step cells wait for the first completed
/// model or tool call so `0 steps` cannot look like a stalled scoreboard.
#[must_use]
pub fn build_groups(snapshot: MetricsSnapshot, locale: Locale) -> Vec<MetricGroupCells> {
    let label = |id: MessageId| tr(locale, id).into_owned();
    let mut groups = Vec::new();
    for group in GROUP_ORDER {
        let cells = match group {
            MetricGroup::Turns => {
                if snapshot.turns == 0 && snapshot.steps == 0 {
                    continue;
                }
                let mut cells = Vec::new();
                if snapshot.turns > 0 {
                    cells.push(MetricCell {
                        label: label(if snapshot.turns == 1 {
                            MessageId::SessionMetricsTurn
                        } else {
                            MessageId::SessionMetricsTurns
                        }),
                        value: snapshot.turns.to_string(),
                        value_first: true,
                    });
                }
                if snapshot.steps > 0 {
                    cells.push(MetricCell {
                        label: label(if snapshot.steps == 1 {
                            MessageId::SessionMetricsStep
                        } else {
                            MessageId::SessionMetricsSteps
                        }),
                        value: snapshot.steps.to_string(),
                        value_first: true,
                    });
                }
                if cells.is_empty() {
                    continue;
                }
                cells
            }
            MetricGroup::Llm => {
                let mut cells = Vec::new();
                if !snapshot.llm_time.is_zero() {
                    cells.push(MetricCell {
                        label: label(MessageId::SessionMetricsLlm),
                        value: format_duration(snapshot.llm_time),
                        value_first: false,
                    });
                }
                if !snapshot.tool_time.is_zero() {
                    cells.push(MetricCell {
                        label: label(MessageId::SessionMetricsTools),
                        value: format_duration(snapshot.tool_time),
                        value_first: false,
                    });
                }
                if cells.is_empty() {
                    continue;
                }
                cells
            }
            MetricGroup::Latency => {
                let mut cells = Vec::new();
                if let Some(ttft) = snapshot.ttft_avg {
                    cells.push(MetricCell {
                        label: label(MessageId::SessionMetricsTtft),
                        value: format_duration(ttft),
                        value_first: false,
                    });
                }
                if let Some(rate) = snapshot.tokens_per_second {
                    cells.push(MetricCell {
                        label: label(MessageId::SessionMetricsTokensPerSecond),
                        value: format_rate(rate),
                        value_first: true,
                    });
                }
                if cells.is_empty() {
                    continue;
                }
                cells
            }
            MetricGroup::Cache => {
                let Some(pct) = snapshot.cache_hit_percent else {
                    continue;
                };
                vec![MetricCell {
                    label: label(MessageId::SessionMetricsCache),
                    value: format!("{pct}%"),
                    value_first: false,
                }]
            }
            MetricGroup::Input => {
                if snapshot.input_tokens == 0 {
                    continue;
                }
                vec![MetricCell {
                    label: label(MessageId::SessionMetricsInput),
                    value: format_tokens(snapshot.input_tokens),
                    value_first: false,
                }]
            }
        };
        groups.push(MetricGroupCells { group, cells });
    }
    groups
}

/// A rendered strip: the plain text (for tests, `/status`, and width math)
/// plus the cells that survived the budget, so the painter can style labels
/// and values differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedStrip {
    pub groups: Vec<MetricGroupCells>,
    pub separators: Separators,
}

impl RenderedStrip {
    /// Plain-text form: `4 turns · 108 steps │ LLM 11m46s · tools 1m52s │ …`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (index, group) in self.groups.iter().enumerate() {
            if index > 0 {
                out.push_str(self.separators.group);
            }
            for (cell_index, cell) in group.cells.iter().enumerate() {
                if cell_index > 0 {
                    out.push_str(self.separators.cell);
                }
                if cell.value_first {
                    out.push_str(&cell.value);
                    out.push(' ');
                    out.push_str(&cell.label);
                } else {
                    out.push_str(&cell.label);
                    out.push(' ');
                    out.push_str(&cell.value);
                }
            }
        }
        out
    }
}

/// Snapshot the live app state into the strip's inputs.
#[must_use]
pub fn snapshot_from_app(app: &crate::tui::app::App) -> MetricsSnapshot {
    let hit = u64::from(app.session.displayed_total_cache_hit_tokens());
    let miss = u64::from(app.session.displayed_total_cache_miss_tokens());
    let cache_hit_percent = (hit + miss > 0).then(|| {
        // Widen before adding so saturated counters never exceed 100%.
        u8::try_from((hit * 100 + (hit + miss) / 2) / (hit + miss)).unwrap_or(100)
    });
    MetricsSnapshot {
        turns: app.turn_counter,
        steps: app.session_metrics.steps(),
        llm_time: app.session_metrics.llm_time,
        tool_time: app.session_metrics.tool_time,
        ttft_avg: app.session_metrics.ttft_average(),
        tokens_per_second: app.session_metrics.tokens_per_second(),
        cache_hit_percent,
        input_tokens: u64::from(app.session.displayed_total_input_tokens()),
    }
}

/// The complete, untrimmed strip text — what `/status` prints.
#[must_use]
pub fn full_text(snapshot: MetricsSnapshot, locale: Locale, ascii_safe: bool) -> String {
    RenderedStrip {
        groups: build_groups(snapshot, locale),
        separators: Separators::for_ascii(ascii_safe),
    }
    .text()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MetricsSnapshot {
        MetricsSnapshot {
            turns: 4,
            steps: 108,
            llm_time: Duration::from_secs(11 * 60 + 46),
            tool_time: Duration::from_secs(60 + 52),
            ttft_avg: Some(Duration::from_millis(1_500)),
            tokens_per_second: Some(120.0),
            cache_hit_percent: Some(99),
            input_tokens: 9_300_000,
        }
    }

    #[test]
    fn durations_format_like_the_harness_strip() {
        assert_eq!(format_duration(Duration::ZERO), "0s");
        assert_eq!(format_duration(Duration::from_millis(320)), "320ms");
        assert_eq!(format_duration(Duration::from_millis(1_500)), "1.5s");
        assert_eq!(format_duration(Duration::from_millis(1_549)), "1.5s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59.0s");
        assert_eq!(format_duration(Duration::from_secs(11 * 60 + 46)), "11m46s");
        assert_eq!(format_duration(Duration::from_secs(3_600 + 120)), "1h02m");
    }

    #[test]
    fn tokens_and_rates_format_compactly() {
        assert_eq!(format_tokens(842), "842");
        assert_eq!(format_tokens(12_345), "12.3K");
        assert_eq!(format_tokens(128_000), "128K");
        assert_eq!(format_tokens(9_300_000), "9.3M");
        assert_eq!(format_tokens(1_200_000_000), "1.2B");
        assert_eq!(format_rate(120.4), "120");
        assert_eq!(format_rate(7.46), "7.5");
    }

    #[test]
    fn full_strip_matches_the_reference_layout() {
        let text = full_text(sample(), Locale::En, false);
        assert_eq!(
            text,
            "4 turns · 108 steps │ LLM 11m46s · Tool call 1m52s │ TTFT avg 1.5s · 120 tok/s │ Cache hit 99% │ Input 9.3M"
        );
        let ascii = full_text(sample(), Locale::En, true);
        assert!(ascii.is_ascii(), "{ascii}");
        assert!(ascii.contains(" | LLM 11m46s . Tool call 1m52s | "));
    }

    #[test]
    fn idle_snapshot_paints_nothing() {
        let text = full_text(MetricsSnapshot::default(), Locale::En, false);
        assert_eq!(text, "");
    }

    #[test]
    fn absent_evidence_omits_the_cell_instead_of_a_placeholder() {
        let mut snapshot = sample();
        snapshot.cache_hit_percent = None;
        snapshot.ttft_avg = None;
        snapshot.tokens_per_second = None;
        snapshot.input_tokens = 0;
        let text = full_text(snapshot, Locale::En, false);
        assert_eq!(text, "4 turns · 108 steps │ LLM 11m46s · Tool call 1m52s");
        assert!(!text.contains('—'), "{text}");

        // A partially reported latency group keeps only the reported cell.
        snapshot.ttft_avg = Some(Duration::from_millis(900));
        let text = full_text(snapshot, Locale::En, false);
        assert!(text.ends_with("│ TTFT avg 900ms"), "{text}");
        snapshot.ttft_avg = None;
        snapshot.tokens_per_second = Some(88.0);
        let text = full_text(snapshot, Locale::En, false);
        assert!(text.ends_with("│ 88 tok/s"), "{text}");
    }

    #[test]
    fn singular_labels_for_one_turn_and_one_step() {
        let snapshot = MetricsSnapshot {
            turns: 1,
            steps: 1,
            ..MetricsSnapshot::default()
        };
        let text = full_text(snapshot, Locale::En, false);
        assert_eq!(text, "1 turn · 1 step", "{text}");
    }

    #[test]
    fn every_shipped_locale_has_short_labels() {
        for locale in Locale::shipped_complete() {
            let text = full_text(sample(), *locale, false);
            assert!(text.contains("4 "), "{}: {text}", locale.tag());
            assert!(text.contains("11m46s"), "{}: {text}", locale.tag());
            for group in build_groups(sample(), *locale) {
                for cell in group.cells {
                    assert!(
                        cell.label.chars().count() <= 12,
                        "{}: label `{}` is too long for the strip",
                        locale.tag(),
                        cell.label
                    );
                }
            }
        }
    }

    #[test]
    fn accumulators_derive_ttft_and_rate_from_reported_calls() {
        let mut metrics = SessionMetrics::default();
        // 100 output tokens over a 2 s stream, first token after 500 ms, whole
        // call 2.4 s including connection setup.
        metrics.record_model_call(100, 2_000, Some(500), Some(2_400));
        // A call that reported no first token (empty response) still counts
        // for LLM time but not for TTFT.
        metrics.record_model_call(20, 1_000, None, Some(1_100));
        assert_eq!(metrics.model_calls, 2);
        assert_eq!(metrics.llm_time, Duration::from_millis(3_500));
        assert_eq!(metrics.ttft_average(), Some(Duration::from_millis(500)));
        let rate = metrics.tokens_per_second().expect("rate");
        assert!((rate - 40.0).abs() < 1e-9, "{rate}");

        // Missing request_ms falls back to the stream duration.
        metrics.record_model_call(0, 700, None, None);
        assert_eq!(metrics.llm_time, Duration::from_millis(4_200));
        // Zero output tokens must not poison the rate.
        assert!((metrics.tokens_per_second().unwrap() - 120.0 / 3.7).abs() < 1e-9);
    }

    #[test]
    fn tool_time_needs_both_edges_and_in_flight_timers_are_dropped() {
        let mut metrics = SessionMetrics::default();
        let t0 = Instant::now();
        metrics.record_tool_started_at("a", t0);
        metrics.record_tool_completed_at("a", t0 + Duration::from_millis(1_500));
        // Completion without a seen start counts the call, not the time.
        metrics.record_tool_completed_at("ghost", t0 + Duration::from_secs(9));
        assert_eq!(metrics.tool_calls, 2);
        assert_eq!(metrics.tool_time, Duration::from_millis(1_500));
        assert_eq!(metrics.steps(), 2);

        metrics.record_tool_started_at("b", t0);
        metrics.clear_in_flight();
        metrics.record_tool_completed_at("b", t0 + Duration::from_secs(5));
        assert_eq!(metrics.tool_time, Duration::from_millis(1_500));
    }
}
