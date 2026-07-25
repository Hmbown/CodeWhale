//! Live phase band for the underwater shell.
//!
//! The HTML reference attaches activity to the transcript and leaves the
//! composer as the final stable object. That means live phases
//! (working / waiting / approval / failed / done) render **above** the
//! composer, while idle and typing keep a quiet phase line beneath it.
//!
//! Classic shell keeps the legacy footer-below-composer order; this module
//! only decides Ocean placement and paints the one-line band.

use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

use crate::localization::{MessageId, tr};
use crate::tui::{
    app::App,
    underwater::{LiveActivity, ShellPhase, ShellTier, phase_marker_with_activity},
};

/// Where the phase band sits relative to the composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStripPlacement {
    /// Live activity: phase sits on the transcript side of the prompt.
    AboveComposer,
    /// Idle / drafting: quiet phase under the prompt.
    BelowComposer,
}

impl PhaseStripPlacement {
    /// Live phases stay above the composer so the prompt is the bottom
    /// stable object. Idle and typing keep the quiet footer under `❯`.
    #[must_use]
    pub fn for_phase(phase: ShellPhase) -> Self {
        match phase {
            ShellPhase::Working
            | ShellPhase::Verifying
            | ShellPhase::Waiting
            | ShellPhase::Approval
            | ShellPhase::Failed
            | ShellPhase::Done => Self::AboveComposer,
            ShellPhase::Idle | ShellPhase::Typing => Self::BelowComposer,
        }
    }

    #[must_use]
    pub fn is_above_composer(self) -> bool {
        matches!(self, Self::AboveComposer)
    }
}

/// Fixed one-row reservation for the phase band.
#[must_use]
pub fn height() -> u16 {
    1
}

fn span_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|span| span.content.width()).sum()
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > width {
            break;
        }
        result.push(ch);
        used += ch_width;
    }
    result.push('…');
    result
}

/// Compact working detail for the phase band: `×N` for tools or `1m 15s`
/// while the model is thinking.
/// Kept quieter than the classic footer's verbose tool-status line so the
/// transcript owns the ledger and the strip only names the live pulse.
fn working_detail(app: &App, activity: LiveActivity) -> Option<String> {
    let running = activity.running_tool_count();
    let secs = app
        .turn_started_at
        .map(|started| started.elapsed().as_secs());
    match (running, secs) {
        (0, Some(secs)) if secs > 0 => Some(crate::elapsed::format_elapsed_secs(secs)),
        (n, Some(_)) if n > 0 => Some(format!("×{n}")),
        (n, None) if n > 0 => Some(format!("×{n}")),
        _ => None,
    }
}

fn session_cache_hit_percentage(app: &App) -> Option<u8> {
    let hit = u64::from(app.session.total_cache_hit_tokens);
    let miss = u64::from(app.session.total_cache_miss_tokens);
    let total = hit + miss;
    if total == 0 {
        return None;
    }

    // Round to the nearest whole percent. Widen before adding so sessions
    // with saturated u32 telemetry counters can never render above 100%.
    Some(((hit * 100 + total / 2) / total) as u8)
}

/// Paint the one-line phase rail. Compact left marker (icon + verb + duration)
/// instead of a full-width routine phase band. Amber only for approval/waiting;
/// cyan/teal for routine work.
pub fn render(area: Rect, buf: &mut Buffer, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let status_toast = app.active_status_toast();
    let activity = LiveActivity::from_app(app);
    let phase = ShellPhase::from_app_with_activity(app, activity);
    let tier = ShellTier::for_chrome_width(area.width);
    // Quiet chrome background — never paint the full row in phase accent.
    Block::default()
        .style(Style::default().bg(app.ui_theme.footer_bg))
        .render(area, buf);

    // Compact left rail: one accent cell + marker + verb (not a full-width band).
    let rail_color = phase.color(app);
    let (marker, phase_label) = phase_marker_with_activity(app, phase, activity);
    let phase_style = Style::default().fg(rail_color).add_modifier(
        if matches!(phase, ShellPhase::Waiting | ShellPhase::Approval) {
            Modifier::BOLD
        } else {
            Modifier::empty()
        },
    );
    let mut left = vec![
        Span::styled("▌", phase_style),
        Span::styled(marker, phase_style),
        Span::raw(" "),
        Span::styled(phase_label.clone(), phase_style),
    ];

    if tier != ShellTier::Compact
        && matches!(phase, ShellPhase::Working | ShellPhase::Verifying)
        && let Some(detail) = working_detail(app, activity)
    {
        left.push(Span::styled(
            " · ",
            Style::default().fg(app.ui_theme.text_dim),
        ));
        left.push(Span::styled(
            detail,
            Style::default().fg(app.ui_theme.status_working),
        ));
    }

    if tier != ShellTier::Compact
        && let Some(toast) = status_toast.filter(|toast| {
            // Completion may land in the same event drain as an approval
            // denial. Keep unresolved attention/error receipts visible after
            // `done`; only routine informational completion copy yields to the
            // stable done marker.
            let survives_completion = matches!(
                toast.level,
                crate::tui::app::StatusToastLevel::Warning
                    | crate::tui::app::StatusToastLevel::Error
            );
            (phase != ShellPhase::Done || survives_completion)
                && !toast.text.trim().is_empty()
                && toast.text.trim() != phase_label.as_ref()
        })
    {
        left.push(Span::styled(
            " · ",
            Style::default().fg(app.ui_theme.text_dim),
        ));
        left.push(Span::styled(
            truncate_to_width(toast.text.trim(), 40),
            Style::default().fg(crate::tui::ui::status_color(toast.level)),
        ));
    }

    let cost = app.displayed_session_cost_for_currency(app.cost_currency);
    let chip = crate::route_billing::usage_chip(
        app.billing_presentation,
        app.api_provider,
        &app.model,
        cost,
        app.cost_currency,
        None,
    );
    if let crate::route_billing::UsageChip::Money(amount) = chip
        && tier != ShellTier::Compact
    {
        left.push(Span::styled(
            " · ",
            Style::default().fg(app.ui_theme.text_dim),
        ));
        left.push(Span::styled(
            amount,
            Style::default().fg(app.ui_theme.text_muted),
        ));
    }

    if tier != ShellTier::Compact
        && app.status_items.contains(&crate::config::StatusItem::Cache)
        && let Some(pct) = session_cache_hit_percentage(app)
    {
        left.push(Span::styled(
            " · ",
            Style::default().fg(app.ui_theme.text_dim),
        ));
        left.push(Span::styled(
            format!("cache {pct}%"),
            Style::default().fg(app.ui_theme.text_muted),
        ));
    }

    // Live phases keep the strip quiet: no detail-key chorus competing with
    // the ledger. Idle/typing may advertise keys on the quiet footer.
    // Hints come from shell_key_routing so advertised chords match handlers;
    // bare letters are never advertised — the composer owns printable keys.
    let right_text: Cow<'static, str> = if PhaseStripPlacement::for_phase(phase).is_above_composer()
    {
        Cow::Borrowed("")
    } else {
        use crate::tui::shell_key_routing::{ShellBindingId, binding, footer_action_hints};
        let hint_keys = tr(app.ui_locale, MessageId::FooterHintKeys);
        let hint_output = tr(app.ui_locale, MessageId::FooterHintOutput);
        let hint_context = tr(app.ui_locale, MessageId::FooterHintContext);
        Cow::Owned(match tier {
            ShellTier::Compact => {
                format!("{}:{hint_keys}", binding(ShellBindingId::Help).footer_chord)
            }
            ShellTier::Normal => footer_action_hints(false)
                .replace("{output}", hint_output.as_ref())
                .replace("{keys}", hint_keys.as_ref()),
            ShellTier::Wide => footer_action_hints(true)
                .replace("{output}", hint_output.as_ref())
                .replace("{context}", hint_context.as_ref())
                .replace("{keys}", hint_keys.as_ref()),
        })
    };

    let right_width = right_text.width();
    let available = usize::from(area.width);
    let left_width = span_width(&left);
    if right_width > 0 && left_width + right_width < available {
        left.push(Span::raw(" ".repeat(available - left_width - right_width)));
        left.push(Span::styled(
            right_text.into_owned(),
            Style::default().fg(app.ui_theme.text_hint),
        ));
    }
    Paragraph::new(Line::from(left)).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        tui::active_cell::ActiveCell,
        tui::app::TuiOptions,
        tui::history::{ExecCell, ExecSource, HistoryCell, ToolCell, ToolStatus},
    };
    use ratatui::{Terminal, backend::TestBackend};
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };

    fn test_app() -> App {
        App::new(
            TuiOptions {
                model: "deepseek-v4-flash".to_string(),
                ..crate::test_support::test_tui_options(PathBuf::from("."))
            },
            &Config::default(),
        )
    }

    #[test]
    fn live_phases_sit_above_composer_idle_stays_below() {
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Working),
            PhaseStripPlacement::AboveComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Waiting),
            PhaseStripPlacement::AboveComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Approval),
            PhaseStripPlacement::AboveComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Failed),
            PhaseStripPlacement::AboveComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Done),
            PhaseStripPlacement::AboveComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Idle),
            PhaseStripPlacement::BelowComposer
        );
        assert_eq!(
            PhaseStripPlacement::for_phase(ShellPhase::Typing),
            PhaseStripPlacement::BelowComposer
        );
    }

    #[test]
    fn working_marker_uses_the_live_work_status_role() {
        let app = test_app();
        assert_eq!(ShellPhase::Working.color(&app), app.ui_theme.status_working);
        assert_ne!(ShellPhase::Working.color(&app), app.ui_theme.info);
    }

    #[test]
    fn working_band_names_tool_use_and_bounded_count_without_key_chorus() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.is_loading = true;
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));
        let mut active = ActiveCell::new();
        active.push_tool(
            "exec-1",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                // A build, not a test run — `cargo test` would truthfully
                // classify as the `verifying` phase (ShellPhase::Verifying).
                command: "cargo build -p tui".to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: app.turn_started_at,
                duration_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            })),
        );
        app.active_cell = Some(active);

        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("using tool"), "{text}");
        assert!(text.contains("×1"), "{text}");
        assert!(
            !text.contains("12s"),
            "tool elapsed time belongs to the live tool row: {text}"
        );
        assert!(
            !text.contains("run ×1"),
            "detail repeated the tool verb: {text}"
        );
        assert!(
            !text.contains("Alt+?") && !text.contains("F1:"),
            "live phase strip stays quiet: {text}"
        );
    }

    #[test]
    fn compact_activity_band_keeps_only_the_semantic_label() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::En;
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));
        let mut active = ActiveCell::new();
        active.push_tool(
            "exec-compact",
            HistoryCell::Tool(ToolCell::Exec(ExecCell {
                command: "cargo build -p tui".to_string(),
                status: ToolStatus::Running,
                output: None,
                live_output: None,
                shell_task_id: None,
                owner_agent_id: None,
                owner_agent_name: None,
                started_at: app.turn_started_at,
                duration_ms: None,
                source: ExecSource::Assistant,
                interaction: None,
                output_summary: None,
            })),
        );
        app.active_cell = Some(active);

        let backend = TestBackend::new(50, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("using tool"), "{text}");
        assert!(
            !text.contains('×'),
            "compact strip leaked count detail: {text}"
        );
        assert!(
            !text.contains("12s"),
            "compact strip leaked timing detail: {text}"
        );
    }

    #[test]
    fn working_band_keeps_elapsed_time_when_model_is_thinking() {
        let mut app = test_app();
        app.turn_started_at = Some(Instant::now() - Duration::from_secs(12));

        assert_eq!(
            working_detail(&app, LiveActivity::from_app(&app)).as_deref(),
            Some("12s")
        );
    }

    #[test]
    fn completed_band_keeps_unresolved_warning_visible() {
        let mut app = test_app();
        app.runtime_turn_status = Some("completed".to_string());
        app.push_status_toast(
            "Auto-denied exec_shell: denied earlier; restart Codewhale",
            crate::tui::app::StatusToastLevel::Warning,
            Some(12_000),
        );

        let backend = TestBackend::new(100, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(text.contains("done"), "completion phase missing: {text}");
        assert!(
            text.contains("Auto-denied exec_shell"),
            "completion hid unresolved warning: {text}"
        );
    }

    #[test]
    fn cache_percentage_uses_wide_arithmetic_and_rounds() {
        let mut app = test_app();
        assert_eq!(session_cache_hit_percentage(&app), None);

        app.session.total_cache_hit_tokens = 2;
        app.session.total_cache_miss_tokens = 1;
        assert_eq!(session_cache_hit_percentage(&app), Some(67));

        app.session.total_cache_hit_tokens = u32::MAX;
        app.session.total_cache_miss_tokens = u32::MAX;
        assert_eq!(session_cache_hit_percentage(&app), Some(50));
    }

    #[test]
    fn cache_chip_is_labeled_configurable_and_hidden_when_compact() {
        let mut app = test_app();
        app.status_items = vec![crate::config::StatusItem::Cache];
        app.session.total_cache_hit_tokens = 7;
        app.session.total_cache_miss_tokens = 3;

        let backend = TestBackend::new(80, 1);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("draw");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(text.contains("cache 70%"), "{text}");

        app.status_items.clear();
        terminal
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("draw without cache");
        let text = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("cache"), "{text}");

        app.status_items = vec![crate::config::StatusItem::Cache];
        let backend = TestBackend::new(50, 1);
        let mut compact = Terminal::new(backend).expect("compact terminal");
        compact
            .draw(|frame| render(frame.area(), frame.buffer_mut(), &mut app))
            .expect("compact draw");
        let text = compact
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!text.contains("cache"), "compact strip: {text}");
    }
}
